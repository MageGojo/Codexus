//! ferry-config：接管 Codex 的 `~/.codex/config.toml`。
//!
//! 能力：
//! - **接管（takeover）**：注入码渡的 `[model_providers.<key>]`，把 `base_url`
//!   指向本地代理、`wire_api = "responses"`，并（可选）将其设为默认 provider。
//! - **原子写入**：写临时文件 -> fsync -> 原子 rename，避免半写损坏。
//! - **备份**：接管前自动备份原文件为 `config.toml.codexferry.bak.<时间戳>`。
//! - **回滚**：从最近（或指定）备份恢复；或仅移除码渡注入的键（release）。
//!
//! 使用 `toml_edit` 解析与写回，**保留用户原有内容、注释与格式**。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{value, Array, DocumentMut, Item, Table, TableLike};

/// 默认注入的 provider key：`[model_providers.codexferry]`。
pub const DEFAULT_PROVIDER_KEY: &str = "codexferry";
/// 备份文件名中缀，用于识别码渡创建的备份。
const BACKUP_INFIX: &str = "codexferry.bak";

/// 上游供应商协议类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApi {
    /// OpenAI Chat Completions 兼容接口。
    Chat,
    /// OpenAI Responses 兼容接口。
    Responses,
}

impl ProviderApi {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
        }
    }
}

/// 供应商分组类型：直连官方 / 第三方中转（relay）。
///
/// 仅用于产品分组与「中转 token 掺假检测」的标记；不影响协议转换逻辑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// 直连供应商官方接口（DeepSeek/Qwen 等）。
    #[default]
    Direct,
    /// 第三方中转 / 聚合站点（OpenAI 兼容代理）。
    Relay,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Relay => "relay",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "relay" | "中转" | "transit" | "proxy" => Self::Relay,
            _ => Self::Direct,
        }
    }
}

/// 模型别名：Codex 侧模型名 -> 上游真实模型名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelAlias {
    pub from: &'static str,
    pub to: &'static str,
}

/// 数据驱动的供应商预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api: ProviderApi,
    pub default_model: &'static str,
    pub api_key_env: &'static [&'static str],
    pub aliases: &'static [ModelAlias],
    /// 该供应商可选模型目录（供 GUI 下拉，用户在 Codex 端直接选真实模型名）。
    pub models: &'static [&'static str],
    /// 分组类型：内置预设均为直连。
    pub kind: ProviderKind,
}

impl ProviderPreset {
    /// 从该供应商约定的环境变量中读取 API Key。
    pub fn api_key_from_env(&self) -> Option<String> {
        self.api_key_env
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
    }
}

const CODEX_ALIASES_DEEPSEEK: &[ModelAlias] = &[
    ModelAlias {
        from: "gpt-5",
        to: "deepseek-chat",
    },
    ModelAlias {
        from: "gpt-5-codex",
        to: "deepseek-chat",
    },
    ModelAlias {
        from: "gpt-5.1-codex",
        to: "deepseek-chat",
    },
];

const CODEX_ALIASES_QWEN: &[ModelAlias] = &[
    ModelAlias {
        from: "gpt-5",
        to: "qwen-plus",
    },
    ModelAlias {
        from: "gpt-5-codex",
        to: "qwen-plus",
    },
    ModelAlias {
        from: "gpt-5.1-codex",
        to: "qwen-plus",
    },
];

const CODEX_ALIASES_KIMI: &[ModelAlias] = &[
    ModelAlias {
        from: "gpt-5",
        to: "moonshot-v1-8k",
    },
    ModelAlias {
        from: "gpt-5-codex",
        to: "moonshot-v1-8k",
    },
    ModelAlias {
        from: "gpt-5.1-codex",
        to: "moonshot-v1-8k",
    },
];

const CODEX_ALIASES_GLM: &[ModelAlias] = &[
    ModelAlias {
        from: "gpt-5",
        to: "glm-4-flash",
    },
    ModelAlias {
        from: "gpt-5-codex",
        to: "glm-4-flash",
    },
    ModelAlias {
        from: "gpt-5.1-codex",
        to: "glm-4-flash",
    },
];

// 各供应商可选模型目录（用于 GUI 下拉；用户在 Codex 端可直接选这些真实模型名，
// 代理对未命中别名的模型透传到上游）。按需补充，不求穷尽。
const MODELS_DEEPSEEK: &[&str] = &["deepseek-chat", "deepseek-reasoner"];
const MODELS_QWEN: &[&str] = &[
    "qwen-plus",
    "qwen-max",
    "qwen-turbo",
    "qwen3-coder-plus",
    "qwen3-max",
];
const MODELS_KIMI: &[&str] = &[
    "moonshot-v1-8k",
    "moonshot-v1-32k",
    "moonshot-v1-128k",
    "kimi-k2-0905-preview",
    "kimi-latest",
];
const MODELS_GLM: &[&str] = &["glm-4.6", "glm-4.5", "glm-4-plus", "glm-4-air", "glm-4-flash"];

const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        api: ProviderApi::Chat,
        default_model: "deepseek-chat",
        api_key_env: &["DEEPSEEK_API_KEY"],
        aliases: CODEX_ALIASES_DEEPSEEK,
        models: MODELS_DEEPSEEK,
        kind: ProviderKind::Direct,
    },
    ProviderPreset {
        id: "qwen",
        name: "通义千问 / Qwen",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        api: ProviderApi::Chat,
        default_model: "qwen-plus",
        api_key_env: &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
        aliases: CODEX_ALIASES_QWEN,
        models: MODELS_QWEN,
        kind: ProviderKind::Direct,
    },
    ProviderPreset {
        id: "kimi",
        name: "Kimi / Moonshot",
        base_url: "https://api.moonshot.cn/v1",
        api: ProviderApi::Chat,
        default_model: "moonshot-v1-8k",
        api_key_env: &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        aliases: CODEX_ALIASES_KIMI,
        models: MODELS_KIMI,
        kind: ProviderKind::Direct,
    },
    ProviderPreset {
        id: "glm",
        name: "智谱 GLM",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        api: ProviderApi::Chat,
        default_model: "glm-4-flash",
        api_key_env: &["ZHIPU_API_KEY", "GLM_API_KEY"],
        aliases: CODEX_ALIASES_GLM,
        models: MODELS_GLM,
        kind: ProviderKind::Direct,
    },
];

/// 内置供应商预设列表。
pub fn provider_presets() -> &'static [ProviderPreset] {
    PROVIDER_PRESETS
}

/// 按 ID 查找供应商预设，大小写不敏感。
pub fn find_provider_preset(id: &str) -> Option<&'static ProviderPreset> {
    let id = id.trim();
    provider_presets()
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
}

/// 自定义供应商持久化文件名（落在 `~/.codexferry/`）。
const CUSTOM_PROVIDERS_FILE: &str = "providers.json";

/// 模型别名（owned 版本，用于自定义供应商与统一视图）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAliasOwned {
    pub from: String,
    pub to: String,
}

/// 用户自定义供应商（持久化，不含任何密钥材料）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: ProviderApi,
    pub default_model: String,
    #[serde(default)]
    pub api_key_env: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<ModelAliasOwned>,
    /// 可选模型目录（供 GUI 下拉；为空时回退到 default_model + 别名目标）。
    #[serde(default)]
    pub models: Vec<String>,
    /// 分组类型：直连 / 中转（默认直连，兼容旧数据）。
    #[serde(default)]
    pub kind: ProviderKind,
}

impl CustomProvider {
    fn normalized(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.base_url = self.base_url.trim().to_string();
        self.default_model = self.default_model.trim().to_string();
        self.api_key_env.retain(|e| !e.trim().is_empty());
        self.aliases
            .retain(|a| !a.from.trim().is_empty() && !a.to.trim().is_empty());
        self.models = self
            .models
            .into_iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        self
    }

    /// 校验字段合法性（id 为 slug、关键字段非空）。
    ///
    /// 注意：**允许与内置预设同 id**——此时该自定义条目会作为内置预设的「覆盖」
    /// （override），让内置供应商也能改 base_url / 模型 / 协议 / 直连·中转 / 别名。
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            bail!("供应商 id 不能为空");
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            bail!("供应商 id 仅支持字母、数字、连字符与下划线");
        }
        if self.name.is_empty() {
            bail!("供应商名称不能为空");
        }
        if self.base_url.is_empty() {
            bail!("base_url 不能为空");
        }
        if self.default_model.is_empty() {
            bail!("default_model 不能为空");
        }
        Ok(())
    }
}

/// 统一供应商视图：内置预设 + 自定义，序列化后供 GUI 使用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: ProviderApi,
    pub default_model: String,
    #[serde(default)]
    pub api_key_env: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<ModelAliasOwned>,
    /// 可选模型目录（供 GUI 下拉）。
    #[serde(default)]
    pub models: Vec<String>,
    /// 分组类型：直连 / 中转。
    #[serde(default)]
    pub kind: ProviderKind,
    /// 是否内置预设（存在同 id 的内置预设即为 true，即使被自定义覆盖）。
    pub builtin: bool,
    /// 是否被用户自定义（纯自定义供应商，或对内置预设做了覆盖）。
    #[serde(default)]
    pub customized: bool,
}

impl ProviderEntry {
    pub fn from_preset(p: &ProviderPreset) -> Self {
        Self {
            id: p.id.to_string(),
            name: p.name.to_string(),
            base_url: p.base_url.to_string(),
            api: p.api,
            default_model: p.default_model.to_string(),
            api_key_env: p.api_key_env.iter().map(|s| s.to_string()).collect(),
            aliases: p
                .aliases
                .iter()
                .map(|a| ModelAliasOwned {
                    from: a.from.to_string(),
                    to: a.to.to_string(),
                })
                .collect(),
            models: p.models.iter().map(|m| m.to_string()).collect(),
            kind: p.kind,
            builtin: true,
            customized: false,
        }
    }

    pub fn from_custom(c: CustomProvider) -> Self {
        Self {
            id: c.id,
            name: c.name,
            base_url: c.base_url,
            api: c.api,
            default_model: c.default_model,
            api_key_env: c.api_key_env,
            aliases: c.aliases,
            models: c.models,
            kind: c.kind,
            builtin: false,
            customized: true,
        }
    }

    /// 从该供应商约定的环境变量中读取 API Key。
    pub fn api_key_from_env(&self) -> Option<String> {
        self.api_key_env
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
    }
}

/// 自定义供应商存储（`~/.codexferry/providers.json`，原子写入）。
#[derive(Clone)]
pub struct ProviderStore {
    path: PathBuf,
}

impl ProviderStore {
    /// 定位文件：优先 `FERRY_PROVIDERS_FILE`，否则 `~/.codexferry/providers.json`。
    pub fn locate() -> Result<Self> {
        let path = match std::env::var_os("FERRY_PROVIDERS_FILE") {
            Some(p) => PathBuf::from(p),
            None => home_dir()?.join(".codexferry").join(CUSTOM_PROVIDERS_FILE),
        };
        Ok(Self { path })
    }

    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取全部自定义供应商（文件不存在或为空返回空列表）。
    pub fn list(&self) -> Result<Vec<CustomProvider>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("读取 {} 失败", self.path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&raw).with_context(|| format!("解析 {} 失败", self.path.display()))
    }

    fn write_all(&self, list: &[CustomProvider]) -> Result<()> {
        let json = serde_json::to_string_pretty(list)?;
        atomic_write(&self.path, &json)
    }

    /// 新增或按 id 覆盖一个自定义供应商。
    pub fn upsert(&self, provider: CustomProvider) -> Result<CustomProvider> {
        let provider = provider.normalized();
        provider.validate()?;
        let mut list = self.list()?;
        if let Some(slot) = list
            .iter_mut()
            .find(|c| c.id.eq_ignore_ascii_case(&provider.id))
        {
            *slot = provider.clone();
        } else {
            list.push(provider.clone());
        }
        self.write_all(&list)?;
        Ok(provider)
    }

    /// 按 id 删除，返回是否删除成功。
    pub fn delete(&self, id: &str) -> Result<bool> {
        let id = id.trim();
        let mut list = self.list()?;
        let before = list.len();
        list.retain(|c| !c.id.eq_ignore_ascii_case(id));
        let removed = list.len() != before;
        if removed {
            self.write_all(&list)?;
        }
        Ok(removed)
    }

    /// 导出全部自定义供应商。
    pub fn export(&self) -> Result<Vec<CustomProvider>> {
        self.list()
    }

    /// 导入：`replace=true` 时整表替换，否则按 id 合并覆盖。返回导入条数。
    pub fn import(&self, providers: Vec<CustomProvider>, replace: bool) -> Result<usize> {
        let mut list = if replace { Vec::new() } else { self.list()? };
        let mut count = 0;
        for provider in providers {
            let provider = provider.normalized();
            provider.validate()?;
            if let Some(slot) = list
                .iter_mut()
                .find(|c| c.id.eq_ignore_ascii_case(&provider.id))
            {
                *slot = provider;
            } else {
                list.push(provider);
            }
            count += 1;
        }
        self.write_all(&list)?;
        Ok(count)
    }
}

/// 内置预设 + 自定义供应商的统一列表（内置在前；自定义同 id 覆盖内置）。
///
/// 覆盖项保留 `builtin = true`（仍是内置供应商，只是字段被改），并置 `customized = true`，
/// 供 GUI 提供「恢复默认」。纯自定义供应商追加在后，`builtin = false`、`customized = true`。
pub fn all_providers(store: &ProviderStore) -> Result<Vec<ProviderEntry>> {
    let mut out: Vec<ProviderEntry> =
        provider_presets().iter().map(ProviderEntry::from_preset).collect();
    for custom in store.list()? {
        if let Some(slot) = out.iter_mut().find(|e| e.id.eq_ignore_ascii_case(&custom.id)) {
            // 覆盖内置预设：保留 builtin 标记，标记为已自定义。
            let mut entry = ProviderEntry::from_custom(custom);
            entry.builtin = true;
            *slot = entry;
        } else {
            out.push(ProviderEntry::from_custom(custom));
        }
    }
    Ok(out)
}

/// 解析供应商：自定义覆盖优先（让内置预设的编辑生效），再回退内置预设。大小写不敏感。
pub fn resolve_provider(store: &ProviderStore, id: &str) -> Result<Option<ProviderEntry>> {
    let trimmed = id.trim();
    if let Some(custom) = store
        .list()?
        .into_iter()
        .find(|c| c.id.eq_ignore_ascii_case(trimmed))
    {
        let mut entry = ProviderEntry::from_custom(custom);
        entry.builtin = find_provider_preset(trimmed).is_some();
        return Ok(Some(entry));
    }
    Ok(find_provider_preset(trimmed).map(ProviderEntry::from_preset))
}

/// 计算某供应商可在 Codex 端选择的模型清单（去重、保序）：
/// 显式 `models` → 内置预设目录（按 id，覆盖项也能继承）→ `default_model` + 别名目标。
pub fn entry_models(entry: &ProviderEntry) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |m: &str| {
        let m = m.trim();
        if !m.is_empty() && !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    };
    for m in &entry.models {
        push(m);
    }
    if let Some(p) = find_provider_preset(&entry.id) {
        for m in p.models {
            push(m);
        }
    }
    push(&entry.default_model);
    for a in &entry.aliases {
        push(&a.to);
    }
    out
}

/// 接管参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeoverParams {
    /// `[model_providers.<provider_key>]` 的键名。
    pub provider_key: String,
    /// provider 的 `name` 字段。
    pub provider_name: String,
    /// 指向本地代理，如 `http://127.0.0.1:15721/v1`。
    pub base_url: String,
    /// 必须为 `responses`：当前 Codex 仅支持 Responses 协议，由本地代理转换到上游。
    pub wire_api: String,
    /// 是否需要 OpenAI 原生鉴权（ChatGPT 登录 / OPENAI_API_KEY）。
    ///
    /// **第三方供应商（DeepSeek 等）必须为 `false`**：否则 Codex 会在未登录 OpenAI 时
    /// 直接拦住（"进不去"）。代理自带上游 Key，不依赖 Codex 侧的 OpenAI 鉴权。
    pub requires_openai_auth: bool,
    /// 直填给 Codex 的 bearer（官方 `experimental_bearer_token`）。代理会忽略其值、
    /// 改用自己存储的上游 Key；这里只是让 Codex 在 `requires_openai_auth=false` 时
    /// 仍带上 Authorization，免去设置环境变量 / 登录。`None` 时用内置占位符。
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// 默认模型别名（写入顶层 `model`）。
    pub model: Option<String>,
    /// 是否把该 provider 设为顶层默认（`model_provider = "<key>"`）。
    pub set_as_default: bool,
}

/// `requires_openai_auth=false` 时注入的占位 bearer（代理忽略其值）。
const DEFAULT_LOCAL_BEARER: &str = "sk-codexferry-local";

impl Default for TakeoverParams {
    fn default() -> Self {
        Self {
            provider_key: DEFAULT_PROVIDER_KEY.to_string(),
            provider_name: "Codexus".to_string(),
            base_url: "http://127.0.0.1:15721/v1".to_string(),
            wire_api: "responses".to_string(),
            requires_openai_auth: false,
            bearer_token: None,
            model: None,
            set_as_default: true,
        }
    }
}

/// 接管结果报告。
#[derive(Debug, Clone)]
pub struct TakeoverReport {
    pub config_path: PathBuf,
    /// 接管前生成的备份（原文件不存在时为 `None`）。
    pub backup_path: Option<PathBuf>,
    pub provider_key: String,
}

/// Codex 顶层偏好（写入 `~/.codex/config.toml` 顶层键，保留其余内容）。
///
/// 语义：每个字段 `Some(非空)` 表示写入；`Some("")` 表示删除该键；`None` 表示不改动。
/// 对照 Codex 官方 config 参考（developers.openai.com/codex/config-reference）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexPreferences {
    /// 顶层 `model`（Codex 侧模型名；代理按别名映射或透传到上游）。
    #[serde(default)]
    pub model: Option<String>,
    /// `model_reasoning_effort`：minimal | low | medium | high | xhigh。
    #[serde(default)]
    pub model_reasoning_effort: Option<String>,
    /// `model_reasoning_summary`：auto | concise | detailed | none。
    #[serde(default)]
    pub model_reasoning_summary: Option<String>,
    /// `model_verbosity`：low | medium | high（GPT-5 Responses API 输出详尽度）。
    #[serde(default)]
    pub model_verbosity: Option<String>,
    /// `approval_policy`：untrusted | on-failure | on-request | never。
    #[serde(default)]
    pub approval_policy: Option<String>,
    /// `sandbox_mode`：read-only | workspace-write | danger-full-access。
    #[serde(default)]
    pub sandbox_mode: Option<String>,
}

/// 一个 Codex profile（独立文件 `$CODEX_HOME/<name>.config.toml`）。
///
/// Codex 0.134+ 用独立 profile 文件做「命名配置层」，文件内用顶层键、按层覆盖 base
/// config，命令行用 `codex --profile <name>` 选择。这里把 profile 的可调键复用
/// `CodexPreferences`（model + 推理/审批/沙箱），与「模型与偏好」一致。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexProfile {
    pub name: String,
    pub prefs: CodexPreferences,
}

/// MCP 服务器传输类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// 本地进程（command/args/env）。
    #[default]
    Stdio,
    /// 远程 Streamable HTTP（url/bearer_token_env_var）。
    Http,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

fn default_true() -> bool {
    true
}

/// 一个 Codex MCP 服务器（`[mcp_servers.<name>]`）。
///
/// 字段对齐 Codex 官方：stdio 用 command/args/env/env_vars/cwd；http 用
/// url/bearer_token_env_var。**密钥一律用环境变量名**（`*_env_var`），不落明文值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServer {
    /// 表键名 `[mcp_servers.<name>]`，slug（字母/数字/-/_）。
    pub name: String,
    #[serde(default)]
    pub transport: McpTransport,
    /// 启用开关（默认 true；为 false 时 Codex 不加载，但保留配置）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    // ---- stdio ----
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// 内联环境变量（字面值）。
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// 从宿主转发的环境变量名。
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    // ---- http ----
    #[serde(default)]
    pub url: Option<String>,
    /// HTTP bearer token 的环境变量名（非明文值）。
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    /// 静态 HTTP 请求头（字面值，随每次 MCP HTTP 请求带上）。
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    /// 由环境变量填充的 HTTP 请求头（值为环境变量名，不落明文）。
    #[serde(default)]
    pub env_http_headers: BTreeMap<String, String>,
    // ---- 通用 ----
    /// 为 true 时，该 MCP 启动失败会让 Codex 启动/恢复直接失败（而非降级）。
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub startup_timeout_sec: Option<f64>,
    #[serde(default)]
    pub tool_timeout_sec: Option<f64>,
}

impl Default for McpServer {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport: McpTransport::Stdio,
            enabled: true,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            env_vars: Vec::new(),
            cwd: None,
            url: None,
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            required: false,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
        }
    }
}

impl McpServer {
    fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.command = self.command.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
        self.url = self.url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        self.cwd = self.cwd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
        self.bearer_token_env_var = self
            .bearer_token_env_var
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty());
        self.env_vars.retain(|e| !e.trim().is_empty());
        self.args.retain(|a| !a.is_empty());
        self.http_headers = clean_string_map(std::mem::take(&mut self.http_headers));
        self.env_http_headers = clean_string_map(std::mem::take(&mut self.env_http_headers));
        self
    }

    /// 校验：name 为 slug；stdio 需 command；http 需 url。
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            bail!("MCP 名称不能为空");
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            bail!("MCP 名称仅支持字母、数字、连字符与下划线");
        }
        match self.transport {
            McpTransport::Stdio => {
                if self.command.as_deref().unwrap_or("").is_empty() {
                    bail!("stdio 类型的 MCP 需要 command");
                }
            }
            McpTransport::Http => {
                if self.url.as_deref().unwrap_or("").is_empty() {
                    bail!("http 类型的 MCP 需要 url");
                }
            }
        }
        Ok(())
    }
}

/// 内置 MCP 模板（供前端一键填充）。
#[derive(Debug, Clone, Serialize)]
pub struct McpTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub server: McpServer,
}

/// 内置 MCP 模板列表（常用社区 MCP，命令对照官方文档）。
pub fn mcp_templates() -> Vec<McpTemplate> {
    fn stdio(name: &str, command: &str, args: &[&str]) -> McpServer {
        McpServer {
            name: name.to_string(),
            command: Some(command.to_string()),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }
    vec![
        McpTemplate {
            id: "fetch".into(),
            name: "Fetch 抓取网页".into(),
            description: "把网页抓取为 markdown,适合让 Codex 读在线文档".into(),
            server: stdio("fetch", "uvx", &["mcp-server-fetch"]),
        },
        McpTemplate {
            id: "filesystem".into(),
            name: "Filesystem 文件系统".into(),
            description: "让 Codex 读写指定目录(把最后一个参数改成你的目录)".into(),
            server: stdio(
                "filesystem",
                "npx",
                &["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"],
            ),
        },
        McpTemplate {
            id: "context7".into(),
            name: "Context7 最新库文档".into(),
            description: "按需注入主流库的最新文档与示例".into(),
            server: stdio("context7", "npx", &["-y", "@upstash/context7-mcp"]),
        },
        McpTemplate {
            id: "sequential-thinking".into(),
            name: "Sequential Thinking".into(),
            description: "结构化分步推理工具".into(),
            server: stdio(
                "sequential-thinking",
                "npx",
                &["-y", "@modelcontextprotocol/server-sequential-thinking"],
            ),
        },
        McpTemplate {
            id: "playwright".into(),
            name: "Playwright 浏览器自动化".into(),
            description: "驱动浏览器做网页操作 / 截图 / 抓取".into(),
            server: stdio("playwright", "npx", &["-y", "@playwright/mcp@latest"]),
        },
        McpTemplate {
            id: "memory".into(),
            name: "Memory 长期记忆".into(),
            description: "给 Codex 一块跨会话的知识图谱记忆".into(),
            server: stdio("memory", "npx", &["-y", "@modelcontextprotocol/server-memory"]),
        },
        McpTemplate {
            id: "time".into(),
            name: "Time 时间与时区".into(),
            description: "查询当前时间、做时区换算".into(),
            server: stdio("time", "uvx", &["mcp-server-time"]),
        },
        McpTemplate {
            id: "github".into(),
            name: "GitHub(远程 HTTP)".into(),
            description: "GitHub 官方远程 MCP,需在环境变量里放 token".into(),
            server: McpServer {
                name: "github".into(),
                transport: McpTransport::Http,
                url: Some("https://api.githubcopilot.com/mcp/".into()),
                bearer_token_env_var: Some("GITHUB_MCP_TOKEN".into()),
                ..Default::default()
            },
        },
    ]
}

/// 批量解析「粘贴的 MCP 配置」为一组 `McpServer`（对照 cc-switch 的「自定义配置导入」）。
///
/// 自动识别两种常见格式，逐个 `normalized()` + `validate()`：
/// - **JSON**：Claude Desktop / 通用格式
///   `{ "mcpServers": { "名称": { command|url, ... } } }`，也接受 `mcp_servers` /
///   `servers` 包裹键，或直接是 `{ "名称": { ... } }` 的名称→配置映射。
///   stdio 用 `command`/`args`/`env`/`env_vars`/`cwd`；http 用 `url`（或 `type`=http/sse）、
///   `headers`/`http_headers`/`env_http_headers`/`bearer_token_env_var`。`disabled:true`
///   等价 `enabled:false`。
/// - **TOML**：Codex `config.toml` 片段，`[mcp_servers.名称]` 子表，或顶层 `[名称]` 表
///   （含 `command` 或 `url`）。
pub fn parse_mcp_import(text: &str) -> Result<Vec<McpServer>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("导入内容为空");
    }
    // 先按 JSON 解析（最常见的 mcpServers 粘贴格式）。
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return parse_mcp_from_json(&v);
    }
    // 退回 TOML（Codex config.toml 片段）。
    if let Ok(doc) = trimmed.parse::<DocumentMut>() {
        return parse_mcp_from_toml(&doc);
    }
    bail!("无法解析：内容既不是合法 JSON，也不是合法 TOML")
}

/// 从 JSON 顶层定位「名称 -> 配置」映射（兼容多种包裹键）。
fn locate_mcp_json_map(
    v: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>> {
    for key in ["mcpServers", "mcp_servers", "servers"] {
        if let Some(m) = v.get(key).and_then(|x| x.as_object()) {
            return Ok(m);
        }
    }
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("JSON 顶层应为对象"))?;
    if obj.contains_key("command") || obj.contains_key("url") {
        bail!("检测到单个 MCP 配置但缺少名称，请用 {{\"mcpServers\": {{\"名称\": {{...}}}}}} 包裹");
    }
    Ok(obj)
}

fn parse_mcp_from_json(v: &serde_json::Value) -> Result<Vec<McpServer>> {
    let map = locate_mcp_json_map(v)?;
    let mut out = Vec::new();
    for (name, cfg) in map {
        let server = mcp_from_json(name, cfg)?.normalized();
        server
            .validate()
            .with_context(|| format!("MCP「{name}」校验失败"))?;
        out.push(server);
    }
    if out.is_empty() {
        bail!("未发现任何 MCP 服务器");
    }
    Ok(out)
}

/// 把单个 JSON 配置对象转为 `McpServer`。
fn mcp_from_json(name: &str, v: &serde_json::Value) -> Result<McpServer> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("MCP「{name}」配置必须是对象"))?;
    let get_str = |k: &str| {
        obj.get(k)
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
    };
    let get_arr = |k: &str| {
        obj.get(k)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(json_scalar_to_string).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let get_map = |k: &str| {
        let mut m = BTreeMap::new();
        if let Some(o) = obj.get(k).and_then(|x| x.as_object()) {
            for (kk, vv) in o {
                if let Some(s) = json_scalar_to_string(vv) {
                    m.insert(kk.clone(), s);
                }
            }
        }
        m
    };
    let get_f64 = |k: &str| obj.get(k).and_then(|x| x.as_f64());

    let type_str = get_str("type")
        .or_else(|| get_str("transport"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let url = get_str("url");
    let is_http = url.is_some()
        || matches!(
            type_str.as_str(),
            "http" | "sse" | "streamable-http" | "streamable_http" | "http-stream"
        );
    let transport = if is_http {
        McpTransport::Http
    } else {
        McpTransport::Stdio
    };

    let enabled = match obj.get("enabled").and_then(|x| x.as_bool()) {
        Some(b) => b,
        None => match obj.get("disabled").and_then(|x| x.as_bool()) {
            Some(d) => !d,
            None => true,
        },
    };

    // headers：兼容 `http_headers` 与 Claude Desktop 风格的 `headers`。
    let mut http_headers = get_map("http_headers");
    if http_headers.is_empty() {
        http_headers = get_map("headers");
    }

    Ok(McpServer {
        name: name.to_string(),
        transport,
        enabled,
        command: get_str("command"),
        args: get_arr("args"),
        env: get_map("env"),
        env_vars: get_arr("env_vars"),
        cwd: get_str("cwd"),
        url,
        bearer_token_env_var: get_str("bearer_token_env_var"),
        http_headers,
        env_http_headers: get_map("env_http_headers"),
        required: obj.get("required").and_then(|x| x.as_bool()).unwrap_or(false),
        startup_timeout_sec: get_f64("startup_timeout_sec"),
        tool_timeout_sec: get_f64("tool_timeout_sec"),
    })
}

/// JSON 标量（字符串/数字/布尔）转字符串；其余类型返回 None。
fn json_scalar_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse_mcp_from_toml(doc: &DocumentMut) -> Result<Vec<McpServer>> {
    let mut out = Vec::new();
    if let Some(servers) = doc.get("mcp_servers").and_then(Item::as_table_like) {
        for (name, item) in servers.iter() {
            if let Some(t) = item.as_table_like() {
                push_validated_toml(&mut out, name, t)?;
            }
        }
    } else {
        // 顶层表当作「名称 -> 配置」（仅取含 command 或 url 的表）。
        for (name, item) in doc.as_table().iter() {
            if let Some(t) = item.as_table_like() {
                if t.get("command").is_some() || t.get("url").is_some() {
                    push_validated_toml(&mut out, name, t)?;
                }
            }
        }
    }
    if out.is_empty() {
        bail!("未发现任何 MCP 服务器（TOML 需 [mcp_servers.名称] 子表，或顶层含 command/url 的 [名称] 表）");
    }
    Ok(out)
}

fn push_validated_toml(out: &mut Vec<McpServer>, name: &str, t: &dyn TableLike) -> Result<()> {
    let server = table_like_to_mcp(name, t).normalized();
    server
        .validate()
        .with_context(|| format!("MCP「{name}」校验失败"))?;
    out.push(server);
    Ok(())
}

/// Codex 配置目录的句柄（`~/.codex` 或 `$CODEX_HOME`）。
#[derive(Clone)]
pub struct CodexConfig {
    home: PathBuf,
}

impl CodexConfig {
    /// 定位 Codex 配置目录：优先 `CODEX_HOME`，否则 `~/.codex`。
    pub fn locate() -> Result<Self> {
        let home = match std::env::var_os("CODEX_HOME") {
            Some(h) => PathBuf::from(h),
            None => home_dir()?.join(".codex"),
        };
        Ok(Self { home })
    }

    /// 指定配置目录（测试与自定义场景）。
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn config_path(&self) -> PathBuf {
        self.home.join("config.toml")
    }

    pub fn exists(&self) -> bool {
        self.config_path().exists()
    }

    /// 读取原始 config.toml 文本（不存在则返回空串）。
    pub fn read_raw(&self) -> Result<String> {
        let p = self.config_path();
        if !p.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&p).with_context(|| format!("读取 {} 失败", p.display()))
    }

    fn load_doc(&self) -> Result<DocumentMut> {
        load_doc_at(&self.config_path())
    }

    /// 备份当前 config.toml（若存在），返回备份路径。
    pub fn backup(&self) -> Result<Option<PathBuf>> {
        let src = self.config_path();
        if !src.exists() {
            return Ok(None);
        }
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
        let backup = self.home.join(format!("config.toml.{BACKUP_INFIX}.{ts}"));
        let content = fs::read_to_string(&src)
            .with_context(|| format!("读取待备份文件 {} 失败", src.display()))?;
        atomic_write(&backup, &content)?;
        tracing::info!("已备份 Codex 配置 -> {}", backup.display());
        Ok(Some(backup))
    }

    /// 接管：备份 -> 注入码渡 provider -> 原子写入。
    pub fn takeover(&self, params: &TakeoverParams) -> Result<TakeoverReport> {
        fs::create_dir_all(&self.home)
            .with_context(|| format!("创建目录 {} 失败", self.home.display()))?;
        let backup_path = self.backup()?;
        let mut doc = self.load_doc()?;
        inject_provider(&mut doc, params);
        atomic_write(&self.config_path(), &doc.to_string())?;
        tracing::info!("已接管 Codex 配置，provider = {}", params.provider_key);
        Ok(TakeoverReport {
            config_path: self.config_path(),
            backup_path,
            provider_key: params.provider_key.clone(),
        })
    }

    /// 列出码渡创建的备份（按时间倒序，最新在前）。
    pub fn list_backups(&self) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        if !self.home.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.home)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("config.toml.") && name.contains(BACKUP_INFIX) {
                out.push(entry.path());
            }
        }
        out.sort();
        out.reverse();
        Ok(out)
    }

    /// 从指定备份恢复 config.toml（原子写入）。
    pub fn restore(&self, backup: &Path) -> Result<()> {
        let content = fs::read_to_string(backup)
            .with_context(|| format!("读取备份 {} 失败", backup.display()))?;
        atomic_write(&self.config_path(), &content)?;
        tracing::info!("已从备份恢复 Codex 配置：{}", backup.display());
        Ok(())
    }

    /// 恢复到最近一次备份；无备份时返回 `false`。
    pub fn restore_latest(&self) -> Result<bool> {
        match self.list_backups()?.into_iter().next() {
            Some(b) => {
                self.restore(&b)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// 取消接管：仅移除码渡注入的 provider 与默认指向，保留其余内容。
    pub fn release(&self, provider_key: &str) -> Result<()> {
        if !self.exists() {
            return Ok(());
        }
        let mut doc = self.load_doc()?;
        remove_provider(&mut doc, provider_key);
        atomic_write(&self.config_path(), &doc.to_string())?;
        tracing::info!("已取消接管，移除 provider = {provider_key}");
        Ok(())
    }

    // ---- OAuth 直连官方：`~/.codex/auth.json` ----

    /// `~/.codex/auth.json` 路径。
    pub fn auth_path(&self) -> PathBuf {
        self.home.join("auth.json")
    }

    /// 读取现有 auth.json 文本（不存在返回空串）。
    pub fn read_auth(&self) -> Result<String> {
        let path = self.auth_path();
        if !path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&path).with_context(|| format!("读取 {} 失败", path.display()))
    }

    /// 备份当前 auth.json（若存在），返回备份路径。
    pub fn backup_auth(&self) -> Result<Option<PathBuf>> {
        let src = self.auth_path();
        if !src.exists() {
            return Ok(None);
        }
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
        let backup = self.home.join(format!("auth.json.{BACKUP_INFIX}.{ts}"));
        let content = fs::read_to_string(&src)
            .with_context(|| format!("读取待备份文件 {} 失败", src.display()))?;
        atomic_write(&backup, &content)?;
        tracing::info!("已备份 Codex auth.json -> {}", backup.display());
        Ok(Some(backup))
    }

    /// 写入 auth.json（OAuth 直连官方）：先备份现有文件，再原子写。
    pub fn write_auth(&self, content: &str) -> Result<Option<PathBuf>> {
        fs::create_dir_all(&self.home)
            .with_context(|| format!("创建目录 {} 失败", self.home.display()))?;
        let backup = self.backup_auth()?;
        atomic_write(&self.auth_path(), content)?;
        tracing::info!("已写入 Codex auth.json（OAuth 直连官方）");
        Ok(backup)
    }

    // ---- Codex 顶层偏好（换模型 / 推理强度 / 审批 / 沙箱）----

    /// 读取 config.toml 顶层偏好。
    pub fn read_preferences(&self) -> Result<CodexPreferences> {
        Ok(prefs_from_doc(&self.load_doc()?))
    }

    /// 写入 config.toml 顶层偏好（原子写入，保留其余内容与注释）。
    pub fn set_preferences(&self, prefs: &CodexPreferences) -> Result<()> {
        fs::create_dir_all(&self.home)
            .with_context(|| format!("创建目录 {} 失败", self.home.display()))?;
        let mut doc = self.load_doc()?;
        apply_all_prefs(&mut doc, prefs);
        atomic_write(&self.config_path(), &doc.to_string())?;
        Ok(())
    }

    // ---- Codex Profiles（独立文件 `<name>.config.toml`，`codex --profile <name>`）----

    /// 列出所有 profile（扫描 `$CODEX_HOME/*.config.toml`，按名排序）。
    pub fn list_profiles(&self) -> Result<Vec<CodexProfile>> {
        let mut out = Vec::new();
        if !self.home.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.home)? {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().into_owned();
            if let Some(name) = fname.strip_suffix(PROFILE_SUFFIX) {
                if name.is_empty() {
                    continue;
                }
                let prefs = prefs_from_doc(&load_doc_at(&entry.path())?);
                out.push(CodexProfile {
                    name: name.to_string(),
                    prefs,
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// 读取单个 profile（文件不存在则返回空偏好）。
    pub fn read_profile(&self, name: &str) -> Result<CodexProfile> {
        let name = name.trim();
        let prefs = prefs_from_doc(&load_doc_at(&self.profile_path(name))?);
        Ok(CodexProfile {
            name: name.to_string(),
            prefs,
        })
    }

    /// 新增或覆盖一个 profile 文件（原子写入，保留该文件其余键与注释）。
    pub fn upsert_profile(&self, name: &str, prefs: &CodexPreferences) -> Result<()> {
        let name = name.trim();
        validate_profile_name(name)?;
        fs::create_dir_all(&self.home)
            .with_context(|| format!("创建目录 {} 失败", self.home.display()))?;
        let path = self.profile_path(name);
        let mut doc = load_doc_at(&path)?;
        apply_all_prefs(&mut doc, prefs);
        atomic_write(&path, &doc.to_string())?;
        tracing::info!("已写入 Codex profile：{name}");
        Ok(())
    }

    /// 删除一个 profile 文件，返回是否删除成功。
    pub fn delete_profile(&self, name: &str) -> Result<bool> {
        let name = name.trim();
        validate_profile_name(name)?;
        let path = self.profile_path(name);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("删除 {} 失败", path.display()))?;
            tracing::info!("已删除 Codex profile：{name}");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn profile_path(&self, name: &str) -> PathBuf {
        self.home.join(format!("{name}{PROFILE_SUFFIX}"))
    }

    // ---- AGENTS.md（全局指令，对标 cc-switch 的 Prompts）----

    /// 全局 `AGENTS.md` 路径（`$CODEX_HOME/AGENTS.md`）。
    pub fn agents_path(&self) -> PathBuf {
        self.home.join("AGENTS.md")
    }

    /// 读取全局 AGENTS.md（不存在返回空串）。
    pub fn read_agents(&self) -> Result<String> {
        let p = self.agents_path();
        if !p.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&p).with_context(|| format!("读取 {} 失败", p.display()))
    }

    /// 写入全局 AGENTS.md（原子写入）；内容为空白时删除该文件保持整洁。
    pub fn write_agents(&self, content: &str) -> Result<()> {
        let p = self.agents_path();
        if content.trim().is_empty() {
            if p.exists() {
                fs::remove_file(&p).with_context(|| format!("删除 {} 失败", p.display()))?;
            }
            return Ok(());
        }
        fs::create_dir_all(&self.home)
            .with_context(|| format!("创建目录 {} 失败", self.home.display()))?;
        atomic_write(&p, content)
    }

    // ---- MCP 服务器（添加插件）----

    /// 列出 config.toml 里的 MCP 服务器（按名称排序）。
    pub fn list_mcp_servers(&self) -> Result<Vec<McpServer>> {
        let doc = self.load_doc()?;
        let mut out = Vec::new();
        if let Some(servers) = doc.get("mcp_servers").and_then(Item::as_table_like) {
            for (name, item) in servers.iter() {
                if let Some(t) = item.as_table_like() {
                    out.push(table_like_to_mcp(name, t));
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// 新增或按 name 覆盖一个 MCP 服务器（原子写入，保留其余内容）。
    pub fn upsert_mcp_server(&self, server: &McpServer) -> Result<()> {
        let server = server.clone().normalized();
        server.validate()?;
        fs::create_dir_all(&self.home)
            .with_context(|| format!("创建目录 {} 失败", self.home.display()))?;
        let mut doc = self.load_doc()?;
        let servers = doc
            .entry("mcp_servers")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .context("mcp_servers 应为表")?;
        servers.set_implicit(true);
        servers.insert(&server.name, Item::Table(mcp_to_table(&server)));
        atomic_write(&self.config_path(), &doc.to_string())?;
        tracing::info!("已写入 MCP 服务器：{}", server.name);
        Ok(())
    }

    /// 按 name 删除 MCP 服务器，返回是否删除成功。
    pub fn delete_mcp_server(&self, name: &str) -> Result<bool> {
        if !self.exists() {
            return Ok(false);
        }
        let mut doc = self.load_doc()?;
        let removed = doc
            .get_mut("mcp_servers")
            .and_then(Item::as_table_mut)
            .map(|s| s.remove(name).is_some())
            .unwrap_or(false);
        if removed {
            atomic_write(&self.config_path(), &doc.to_string())?;
            tracing::info!("已删除 MCP 服务器：{name}");
        }
        Ok(removed)
    }

    /// 启用 / 停用 MCP 服务器（写 `enabled` 字段），返回是否命中。
    pub fn set_mcp_enabled(&self, name: &str, enabled: bool) -> Result<bool> {
        if !self.exists() {
            return Ok(false);
        }
        let mut doc = self.load_doc()?;
        let mut found = false;
        if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) {
            if let Some(item) = servers.get_mut(name) {
                if let Some(t) = item.as_table_mut() {
                    t["enabled"] = value(enabled);
                    found = true;
                }
            }
        }
        if found {
            atomic_write(&self.config_path(), &doc.to_string())?;
        }
        Ok(found)
    }
}

/// 注入 `[model_providers.<key>]` 并（可选）设为默认。
fn inject_provider(doc: &mut DocumentMut, p: &TakeoverParams) {
    let providers = doc
        .entry("model_providers")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("model_providers 应为表");
    providers.set_implicit(true);

    let mut t = Table::new();
    t["name"] = value(p.provider_name.as_str());
    t["base_url"] = value(p.base_url.as_str());
    t["wire_api"] = value(p.wire_api.as_str());
    t["requires_openai_auth"] = value(p.requires_openai_auth);
    // 本地代理只暴露 HTTP 的 /v1/responses，不提供 WebSocket 通道；显式声明
    // supports_websockets=false，阻止新版 Codex 对本地路由尝试 responses_websocket
    // 通道而反复重连（对照 cockpit-tools issue #754：中转/兼容端点需禁用 WS）。
    t["supports_websockets"] = value(false);
    // 第三方供应商（requires_openai_auth=false）注入占位 bearer，让 Codex 无需
    // OpenAI 登录 / 环境变量即可发请求；代理忽略其值，改用自身存储的上游 Key。
    if !p.requires_openai_auth {
        let bearer = p
            .bearer_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_LOCAL_BEARER);
        t["experimental_bearer_token"] = value(bearer);
    }
    providers.insert(p.provider_key.as_str(), Item::Table(t));

    if p.set_as_default {
        doc["model_provider"] = value(p.provider_key.as_str());
        if let Some(m) = &p.model {
            doc["model"] = value(m.as_str());
        }
    }
}

/// 移除码渡注入的 provider 与默认指向。
fn remove_provider(doc: &mut DocumentMut, key: &str) {
    if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
        providers.remove(key);
    }
    if doc.get("model_provider").and_then(Item::as_str) == Some(key) {
        doc.remove("model_provider");
    }
}

/// profile 文件后缀：`<name>.config.toml`（与官方 `--profile` 约定一致）。
const PROFILE_SUFFIX: &str = ".config.toml";

/// 应用单个顶层偏好：`Some(非空)` 写入、`Some("")` 删除、`None` 不动。
fn apply_pref(doc: &mut DocumentMut, key: &str, v: &Option<String>) {
    match v {
        None => {}
        Some(s) if s.trim().is_empty() => {
            doc.as_table_mut().remove(key);
        }
        Some(s) => {
            doc[key] = value(s.trim());
        }
    }
}

/// 把整套 `CodexPreferences` 应用到文档（顶层键），供 config.toml 与 profile 文件共用。
fn apply_all_prefs(doc: &mut DocumentMut, prefs: &CodexPreferences) {
    apply_pref(doc, "model", &prefs.model);
    apply_pref(doc, "model_reasoning_effort", &prefs.model_reasoning_effort);
    apply_pref(doc, "model_reasoning_summary", &prefs.model_reasoning_summary);
    apply_pref(doc, "model_verbosity", &prefs.model_verbosity);
    apply_pref(doc, "approval_policy", &prefs.approval_policy);
    apply_pref(doc, "sandbox_mode", &prefs.sandbox_mode);
}

/// 从文档解析出顶层偏好（config.toml 与 profile 文件共用）。
fn prefs_from_doc(doc: &DocumentMut) -> CodexPreferences {
    let get = |k: &str| doc.get(k).and_then(Item::as_str).map(|s| s.to_string());
    CodexPreferences {
        model: get("model"),
        model_reasoning_effort: get("model_reasoning_effort"),
        model_reasoning_summary: get("model_reasoning_summary"),
        model_verbosity: get("model_verbosity"),
        approval_policy: get("approval_policy"),
        sandbox_mode: get("sandbox_mode"),
    }
}

/// 解析任意路径的 TOML 文档（不存在或为空返回空文档）。
fn load_doc_at(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    if raw.trim().is_empty() {
        Ok(DocumentMut::new())
    } else {
        raw.parse::<DocumentMut>()
            .with_context(|| format!("解析 {} 失败", path.display()))
    }
}

/// 校验 profile 名称：非空 slug（字母/数字/-/_）。
fn validate_profile_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("profile 名称不能为空");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        bail!("profile 名称仅支持字母、数字、连字符与下划线");
    }
    Ok(())
}

/// 把 `McpServer` 渲染为 TOML 表（标量/数组在前，`env` 子表最后，保证 TOML 合法）。
fn mcp_to_table(s: &McpServer) -> Table {
    let mut t = Table::new();
    t["enabled"] = value(s.enabled);
    if s.required {
        t["required"] = value(true);
    }
    match s.transport {
        McpTransport::Stdio => {
            if let Some(cmd) = &s.command {
                t["command"] = value(cmd.as_str());
            }
            if !s.args.is_empty() {
                let mut arr = Array::new();
                for a in &s.args {
                    arr.push(a.as_str());
                }
                t["args"] = value(arr);
            }
            if !s.env_vars.is_empty() {
                let mut arr = Array::new();
                for e in &s.env_vars {
                    arr.push(e.as_str());
                }
                t["env_vars"] = value(arr);
            }
            if let Some(cwd) = &s.cwd {
                t["cwd"] = value(cwd.as_str());
            }
        }
        McpTransport::Http => {
            if let Some(url) = &s.url {
                t["url"] = value(url.as_str());
            }
            if let Some(b) = &s.bearer_token_env_var {
                t["bearer_token_env_var"] = value(b.as_str());
            }
        }
    }
    if let Some(x) = s.startup_timeout_sec {
        t["startup_timeout_sec"] = value(x);
    }
    if let Some(x) = s.tool_timeout_sec {
        t["tool_timeout_sec"] = value(x);
    }
    // 子表必须最后插入：同一张表里标量键须在子表之前，否则 TOML 非法。
    if matches!(s.transport, McpTransport::Stdio) && !s.env.is_empty() {
        t["env"] = Item::Table(string_map_table(&s.env));
    }
    if matches!(s.transport, McpTransport::Http) {
        if !s.http_headers.is_empty() {
            t["http_headers"] = Item::Table(string_map_table(&s.http_headers));
        }
        if !s.env_http_headers.is_empty() {
            t["env_http_headers"] = Item::Table(string_map_table(&s.env_http_headers));
        }
    }
    t
}

/// 把 `BTreeMap<String, String>` 渲染为 TOML 子表（值均为字符串字面值）。
fn string_map_table(map: &BTreeMap<String, String>) -> Table {
    let mut t = Table::new();
    for (k, v) in map {
        t[k.as_str()] = value(v.as_str());
    }
    t
}

/// 清洗字符串 map：去除键为空白的条目，并裁剪键的首尾空白。
fn clean_string_map(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.into_iter()
        .filter_map(|(k, v)| {
            let k = k.trim().to_string();
            if k.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

/// 从 TOML 表解析出 `McpServer`（含 url 则判为 http，否则 stdio）。
fn table_like_to_mcp(name: &str, t: &dyn TableLike) -> McpServer {
    let get_str = |k: &str| t.get(k).and_then(Item::as_str).map(|s| s.to_string());
    let get_arr = |k: &str| {
        t.get(k)
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let get_f64 = |k: &str| {
        t.get(k)
            .and_then(|i| i.as_float().or_else(|| i.as_integer().map(|x| x as f64)))
    };
    let url = get_str("url");
    let transport = if url.is_some() {
        McpTransport::Http
    } else {
        McpTransport::Stdio
    };
    let enabled = t.get("enabled").and_then(Item::as_bool).unwrap_or(true);
    let required = t.get("required").and_then(Item::as_bool).unwrap_or(false);
    let read_map = |k: &str| {
        let mut m = BTreeMap::new();
        if let Some(sub) = t.get(k).and_then(Item::as_table_like) {
            for (kk, vv) in sub.iter() {
                if let Some(s) = vv.as_str() {
                    m.insert(kk.to_string(), s.to_string());
                }
            }
        }
        m
    };
    McpServer {
        name: name.to_string(),
        transport,
        enabled,
        command: get_str("command"),
        args: get_arr("args"),
        env: read_map("env"),
        env_vars: get_arr("env_vars"),
        cwd: get_str("cwd"),
        url,
        bearer_token_env_var: get_str("bearer_token_env_var"),
        http_headers: read_map("http_headers"),
        env_http_headers: read_map("env_http_headers"),
        required,
        startup_timeout_sec: get_f64("startup_timeout_sec"),
        tool_timeout_sec: get_f64("tool_timeout_sec"),
    }
}

/// 原子写入：写临时文件 -> fsync -> 原子 rename。
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let dir = path.parent().context("目标路径缺少父目录")?;
    fs::create_dir_all(dir)?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config.toml");
    let tmp = dir.join(format!(".{file_name}.tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("创建临时文件 {} 失败", tmp.display()))?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("原子替换 {} 失败", path.display()))?;
    Ok(())
}

/// 应用设置文件名（落在 `~/.codexferry/`）。**不含任何密钥**。
const SETTINGS_FILE: &str = "settings.json";

/// 应用级设置（非密钥）。密钥（如 apizero Key）单独进 Keychain。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    /// 天气城市（手动指定；自动定位关闭时生效，空则仍自动定位）。
    #[serde(default)]
    pub weather_city: String,
    /// 是否自动定位当前城市（默认 true，优先于 weather_city；按本机出口 IP 定位）。
    #[serde(default = "default_true")]
    pub weather_auto_locate: bool,
    /// 是否在界面展示天气卡。
    pub show_weather: bool,
    /// 是否展示古诗词点缀。
    pub show_poem: bool,
    /// 古诗词主题（空=随机）。见 apizero shici 的 type 取值。
    #[serde(default)]
    pub poem_category: String,
    /// 账号池是否启用配额感知调度（优先剩余额度多的账号）。
    pub pool_quota_aware: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            weather_city: String::new(),
            weather_auto_locate: true,
            show_weather: true,
            show_poem: true,
            poem_category: String::new(),
            pool_quota_aware: true,
        }
    }
}

/// 应用设置存储（`~/.codexferry/settings.json`，原子写入，不含密钥）。
#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// 定位文件：优先 `FERRY_SETTINGS_FILE`，否则 `~/.codexferry/settings.json`。
    pub fn locate() -> Result<Self> {
        let path = match std::env::var_os("FERRY_SETTINGS_FILE") {
            Some(p) => PathBuf::from(p),
            None => home_dir()?.join(".codexferry").join(SETTINGS_FILE),
        };
        Ok(Self { path })
    }

    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取设置（文件不存在或损坏时返回默认值）。
    pub fn load(&self) -> AppSettings {
        if !self.path.exists() {
            return AppSettings::default();
        }
        match fs::read_to_string(&self.path) {
            Ok(raw) if !raw.trim().is_empty() => {
                serde_json::from_str(&raw).unwrap_or_else(|e| {
                    tracing::warn!("解析 {} 失败，使用默认设置: {e}", self.path.display());
                    AppSettings::default()
                })
            }
            _ => AppSettings::default(),
        }
    }

    /// 保存设置（原子写入）。
    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        let json = serde_json::to_string_pretty(settings)?;
        atomic_write(&self.path, &json)
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("无法确定用户主目录（HOME / USERPROFILE 均未设置）")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home() -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("ferry-config-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn built_in_provider_presets_cover_mvp_vendors() {
        let ids: Vec<&str> = provider_presets().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["deepseek", "qwen", "kimi", "glm"]);

        let qwen = find_provider_preset("QWEN").unwrap();
        assert_eq!(
            qwen.base_url,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(qwen.default_model, "qwen-plus");
        assert_eq!(qwen.api, ProviderApi::Chat);
        assert!(qwen.api_key_env.contains(&"DASHSCOPE_API_KEY"));
    }

    #[test]
    fn entry_models_includes_provider_catalog() {
        // 内置 deepseek：目录含真实模型名（含 deepseek-reasoner）。
        let ds = ProviderEntry::from_preset(find_provider_preset("deepseek").unwrap());
        let models = entry_models(&ds);
        assert!(models.contains(&"deepseek-chat".to_string()));
        assert!(models.contains(&"deepseek-reasoner".to_string()));

        // 覆盖内置（自定义同 id 且 models 为空）仍能继承内置目录。
        let mut override_ds = ProviderEntry::from_custom(sample_custom("deepseek"));
        override_ds.id = "deepseek".to_string();
        override_ds.models.clear();
        let m2 = entry_models(&override_ds);
        assert!(m2.contains(&"deepseek-reasoner".to_string()), "覆盖项应继承内置目录");

        // 纯自定义、无目录：回退 default_model + 别名目标。
        let custom = ProviderEntry::from_custom(sample_custom("myvendor"));
        let m3 = entry_models(&custom);
        assert!(m3.contains(&"demo-model".to_string()));
    }

    #[test]
    fn provider_presets_map_codex_aliases() {
        let deepseek = find_provider_preset("deepseek").unwrap();
        let alias = deepseek
            .aliases
            .iter()
            .find(|a| a.from == "gpt-5-codex")
            .unwrap();
        assert_eq!(alias.to, "deepseek-chat");
    }

    #[test]
    fn takeover_on_empty_config() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        let report = cfg.takeover(&TakeoverParams::default()).unwrap();
        assert!(report.backup_path.is_none(), "原文件不存在不应有备份");

        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("[model_providers.codexferry]"));
        assert!(raw.contains("base_url = \"http://127.0.0.1:15721/v1\""));
        assert!(raw.contains("wire_api = \"responses\""));
        assert!(raw.contains("model_provider = \"codexferry\""));
        // 第三方供应商默认免 OpenAI 鉴权，并注入占位 bearer 让 Codex 直接发请求。
        assert!(raw.contains("requires_openai_auth = false"));
        assert!(raw.contains("experimental_bearer_token = \"sk-codexferry-local\""));
        // 本地代理无 WebSocket，必须显式禁用，避免 Codex 走 responses_websocket。
        assert!(raw.contains("supports_websockets = false"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn takeover_with_openai_auth_skips_bearer() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        let params = TakeoverParams {
            requires_openai_auth: true,
            ..Default::default()
        };
        cfg.takeover(&params).unwrap();
        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("requires_openai_auth = true"));
        // OpenAI 鉴权模式（账号池/ChatGPT）不注入占位 bearer。
        assert!(!raw.contains("experimental_bearer_token"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn takeover_backs_up_and_preserves_existing() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        fs::write(
            cfg.config_path(),
            "model = \"gpt-5\"\n\n[model_providers.openai]\nname = \"OpenAI\"\n",
        )
        .unwrap();

        let params = TakeoverParams {
            model: Some("gpt-5-codex".to_string()),
            ..Default::default()
        };
        let report = cfg.takeover(&params).unwrap();
        assert!(report.backup_path.is_some(), "原文件存在应生成备份");

        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("[model_providers.codexferry]"));
        assert!(
            raw.contains("[model_providers.openai]"),
            "应保留用户原有 provider"
        );
        assert!(raw.contains("name = \"OpenAI\""));
        assert!(raw.contains("model = \"gpt-5-codex\""));

        // 备份应为原始内容
        let backups = cfg.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
        let backup_raw = fs::read_to_string(&backups[0]).unwrap();
        assert!(backup_raw.contains("model = \"gpt-5\""));
        assert!(!backup_raw.contains("codexferry"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn restore_latest_recovers_original() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        fs::write(cfg.config_path(), "model = \"gpt-5\"\n").unwrap();
        cfg.takeover(&TakeoverParams::default()).unwrap();
        assert!(cfg.read_raw().unwrap().contains("codexferry"));

        assert!(cfg.restore_latest().unwrap());
        let restored = cfg.read_raw().unwrap();
        assert!(!restored.contains("codexferry"), "恢复后不应残留注入");
        assert!(restored.contains("model = \"gpt-5\""));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn release_removes_only_injection() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        fs::write(
            cfg.config_path(),
            "[model_providers.openai]\nname = \"OpenAI\"\n",
        )
        .unwrap();
        cfg.takeover(&TakeoverParams::default()).unwrap();
        cfg.release(DEFAULT_PROVIDER_KEY).unwrap();

        let raw = cfg.read_raw().unwrap();
        assert!(!raw.contains("codexferry"), "应移除码渡注入");
        assert!(raw.contains("openai"), "应保留用户原有 provider");
        fs::remove_dir_all(&home).ok();
    }

    fn sample_custom(id: &str) -> CustomProvider {
        CustomProvider {
            id: id.to_string(),
            name: format!("Custom {id}"),
            base_url: "https://example.com/v1".to_string(),
            api: ProviderApi::Chat,
            default_model: "demo-model".to_string(),
            api_key_env: vec!["DEMO_API_KEY".to_string()],
            aliases: vec![ModelAliasOwned {
                from: "gpt-5-codex".to_string(),
                to: "demo-model".to_string(),
            }],
            models: Vec::new(),
            kind: ProviderKind::Direct,
        }
    }

    #[test]
    fn provider_store_upsert_list_delete_roundtrip() {
        let home = temp_home();
        let store = ProviderStore::with_path(home.join("providers.json"));
        assert!(store.list().unwrap().is_empty());

        store.upsert(sample_custom("myvendor")).unwrap();
        let mut updated = sample_custom("myvendor");
        updated.name = "已更名".to_string();
        store.upsert(updated).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1, "同 id 应覆盖而非新增");
        assert_eq!(list[0].name, "已更名");

        assert!(store.delete("MYVENDOR").unwrap(), "删除应大小写不敏感");
        assert!(store.list().unwrap().is_empty());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn provider_store_allows_builtin_override_rejects_bad_id() {
        let home = temp_home();
        let store = ProviderStore::with_path(home.join("providers.json"));
        // 现在允许与内置预设同 id（作为覆盖），便于编辑内置供应商。
        let mut override_ds = sample_custom("deepseek");
        override_ds.id = "deepseek".to_string();
        override_ds.base_url = "https://relay.example.com/v1".to_string();
        override_ds.kind = ProviderKind::Relay;
        assert!(store.upsert(override_ds).is_ok(), "应允许覆盖内置预设");

        // 非法 id 仍应被拒绝。
        let mut bad = sample_custom("bad id!");
        bad.id = "bad id!".to_string();
        assert!(store.upsert(bad).is_err(), "非法 id 应被拒绝");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn builtin_override_takes_effect_in_views() {
        let home = temp_home();
        let store = ProviderStore::with_path(home.join("providers.json"));
        let mut override_ds = sample_custom("deepseek");
        override_ds.id = "deepseek".to_string();
        override_ds.name = "DeepSeek（中转）".to_string();
        override_ds.base_url = "https://relay.example.com/v1".to_string();
        override_ds.kind = ProviderKind::Relay;
        store.upsert(override_ds).unwrap();

        // resolve 走覆盖：base_url/kind 生效，仍标记 builtin。
        let resolved = resolve_provider(&store, "deepseek").unwrap().unwrap();
        assert_eq!(resolved.base_url, "https://relay.example.com/v1");
        assert_eq!(resolved.kind, ProviderKind::Relay);
        assert!(resolved.builtin, "覆盖项仍属内置供应商");
        assert!(resolved.customized, "应标记为已自定义");

        // all_providers 不应因覆盖而增加条目数（替换而非新增）。
        let all = all_providers(&store).unwrap();
        assert_eq!(all.len(), provider_presets().len());
        let ds = all.iter().find(|e| e.id == "deepseek").unwrap();
        assert_eq!(ds.name, "DeepSeek（中转）");
        assert!(ds.builtin && ds.customized);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn resolve_provider_prefers_builtin_then_custom() {
        let home = temp_home();
        let store = ProviderStore::with_path(home.join("providers.json"));
        store.upsert(sample_custom("myvendor")).unwrap();

        let builtin = resolve_provider(&store, "deepseek").unwrap().unwrap();
        assert!(builtin.builtin);
        assert_eq!(builtin.id, "deepseek");

        let custom = resolve_provider(&store, "myvendor").unwrap().unwrap();
        assert!(!custom.builtin);
        assert_eq!(custom.default_model, "demo-model");

        assert!(resolve_provider(&store, "nope").unwrap().is_none());

        let all = all_providers(&store).unwrap();
        assert_eq!(all.len(), provider_presets().len() + 1);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn provider_store_import_export() {
        let home = temp_home();
        let store = ProviderStore::with_path(home.join("providers.json"));
        store.upsert(sample_custom("keep")).unwrap();

        let imported = store
            .import(vec![sample_custom("a"), sample_custom("b")], false)
            .unwrap();
        assert_eq!(imported, 2);
        assert_eq!(store.export().unwrap().len(), 3, "合并模式应保留已有");

        let replaced = store.import(vec![sample_custom("only")], true).unwrap();
        assert_eq!(replaced, 1);
        let exported = store.export().unwrap();
        assert_eq!(exported.len(), 1, "替换模式应整表覆盖");
        assert_eq!(exported[0].id, "only");
        fs::remove_dir_all(&home).ok();
    }

    // ---- Codex 偏好（换模型）----

    #[test]
    fn set_and_read_preferences_roundtrip_preserves_other_keys() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        fs::write(
            cfg.config_path(),
            "# 用户注释\nmodel = \"old\"\n\n[model_providers.openai]\nname = \"OpenAI\"\n",
        )
        .unwrap();

        cfg.set_preferences(&CodexPreferences {
            model: Some("gpt-5-codex".into()),
            model_reasoning_effort: Some("high".into()),
            model_reasoning_summary: Some("detailed".into()),
            model_verbosity: Some("low".into()),
            approval_policy: Some("on-request".into()),
            sandbox_mode: Some("workspace-write".into()),
        })
        .unwrap();

        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("model = \"gpt-5-codex\""));
        assert!(raw.contains("model_reasoning_effort = \"high\""));
        assert!(raw.contains("model_reasoning_summary = \"detailed\""));
        assert!(raw.contains("model_verbosity = \"low\""));
        assert!(raw.contains("approval_policy = \"on-request\""));
        assert!(raw.contains("sandbox_mode = \"workspace-write\""));
        // 保留用户原有内容与注释。
        assert!(raw.contains("# 用户注释"));
        assert!(raw.contains("[model_providers.openai]"));

        let prefs = cfg.read_preferences().unwrap();
        assert_eq!(prefs.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(prefs.model_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(prefs.model_reasoning_summary.as_deref(), Some("detailed"));
        assert_eq!(prefs.model_verbosity.as_deref(), Some("low"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn set_preferences_none_keeps_some_empty_clears() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        cfg.set_preferences(&CodexPreferences {
            model: Some("m1".into()),
            model_reasoning_effort: Some("low".into()),
            ..Default::default()
        })
        .unwrap();
        // None 不改动 model；Some("") 清除 reasoning。
        cfg.set_preferences(&CodexPreferences {
            model: None,
            model_reasoning_effort: Some(String::new()),
            ..Default::default()
        })
        .unwrap();
        let prefs = cfg.read_preferences().unwrap();
        assert_eq!(prefs.model.as_deref(), Some("m1"), "None 不应改动");
        assert!(prefs.model_reasoning_effort.is_none(), "空串应清除");
        fs::remove_dir_all(&home).ok();
    }

    // ---- MCP 插件 ----

    #[test]
    fn mcp_stdio_upsert_list_toggle_delete() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        let mut srv = McpServer {
            name: "fetch".into(),
            command: Some("uvx".into()),
            args: vec!["mcp-server-fetch".into()],
            ..Default::default()
        };
        srv.env.insert("LOG".into(), "info".into());
        srv.env_vars.push("MY_TOKEN".into());
        srv.startup_timeout_sec = Some(20.0);
        cfg.upsert_mcp_server(&srv).unwrap();

        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("[mcp_servers.fetch]"));
        assert!(raw.contains("command = \"uvx\""));
        assert!(raw.contains("[mcp_servers.fetch.env]"));
        assert!(raw.contains("LOG = \"info\""));

        let list = cfg.list_mcp_servers().unwrap();
        assert_eq!(list.len(), 1);
        let got = &list[0];
        assert_eq!(got.name, "fetch");
        assert_eq!(got.transport, McpTransport::Stdio);
        assert_eq!(got.command.as_deref(), Some("uvx"));
        assert_eq!(got.args, vec!["mcp-server-fetch".to_string()]);
        assert_eq!(got.env.get("LOG").map(String::as_str), Some("info"));
        assert_eq!(got.env_vars, vec!["MY_TOKEN".to_string()]);
        assert_eq!(got.startup_timeout_sec, Some(20.0));
        assert!(got.enabled);

        assert!(cfg.set_mcp_enabled("fetch", false).unwrap());
        assert!(!cfg.list_mcp_servers().unwrap()[0].enabled);

        assert!(cfg.delete_mcp_server("fetch").unwrap());
        assert!(cfg.list_mcp_servers().unwrap().is_empty());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn mcp_http_roundtrip_and_preserves_existing() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        // 先放一个用户已有的接管 provider，确认 MCP 写入不破坏它。
        cfg.takeover(&TakeoverParams::default()).unwrap();

        let srv = McpServer {
            name: "github".into(),
            transport: McpTransport::Http,
            url: Some("https://api.githubcopilot.com/mcp/".into()),
            bearer_token_env_var: Some("GITHUB_MCP_TOKEN".into()),
            ..Default::default()
        };
        cfg.upsert_mcp_server(&srv).unwrap();

        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("[mcp_servers.github]"));
        assert!(raw.contains("url = \"https://api.githubcopilot.com/mcp/\""));
        assert!(raw.contains("bearer_token_env_var = \"GITHUB_MCP_TOKEN\""));
        // 接管的 provider 仍在。
        assert!(raw.contains("[model_providers.codexferry]"));

        let got = &cfg.list_mcp_servers().unwrap()[0];
        assert_eq!(got.transport, McpTransport::Http);
        assert_eq!(got.url.as_deref(), Some("https://api.githubcopilot.com/mcp/"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn mcp_validate_rejects_bad() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        // 空 command 的 stdio。
        let bad = McpServer {
            name: "x".into(),
            ..Default::default()
        };
        assert!(cfg.upsert_mcp_server(&bad).is_err());
        // 非法 name。
        let bad2 = McpServer {
            name: "bad name".into(),
            command: Some("echo".into()),
            ..Default::default()
        };
        assert!(cfg.upsert_mcp_server(&bad2).is_err());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn mcp_templates_are_valid() {
        for t in mcp_templates() {
            assert!(t.server.validate().is_ok(), "模板 {} 应通过校验", t.id);
        }
    }

    #[test]
    fn mcp_http_headers_and_required_roundtrip() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        let mut srv = McpServer {
            name: "relay".into(),
            transport: McpTransport::Http,
            url: Some("https://mcp.example.com/mcp".into()),
            required: true,
            ..Default::default()
        };
        srv.http_headers.insert("X-Source".into(), "codexferry".into());
        srv.env_http_headers
            .insert("Authorization".into(), "RELAY_TOKEN".into());
        cfg.upsert_mcp_server(&srv).unwrap();

        let raw = cfg.read_raw().unwrap();
        assert!(raw.contains("[mcp_servers.relay]"));
        assert!(raw.contains("required = true"));
        assert!(raw.contains("[mcp_servers.relay.http_headers]"));
        assert!(raw.contains("X-Source = \"codexferry\""));
        assert!(raw.contains("[mcp_servers.relay.env_http_headers]"));
        assert!(raw.contains("Authorization = \"RELAY_TOKEN\""));

        let got = &cfg.list_mcp_servers().unwrap()[0];
        assert_eq!(got.transport, McpTransport::Http);
        assert!(got.required);
        assert_eq!(got.http_headers.get("X-Source").map(String::as_str), Some("codexferry"));
        assert_eq!(
            got.env_http_headers.get("Authorization").map(String::as_str),
            Some("RELAY_TOKEN")
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn parse_mcp_import_json_claude_desktop_format() {
        let text = r#"{
            "mcpServers": {
                "fetch": { "command": "uvx", "args": ["mcp-server-fetch"], "env": { "LOG": "info" } },
                "ctx": { "type": "http", "url": "https://ctx.example.com/mcp",
                         "headers": { "X-Key": "v" }, "disabled": true }
            }
        }"#;
        let servers = parse_mcp_import(text).unwrap();
        assert_eq!(servers.len(), 2);
        let fetch = servers.iter().find(|s| s.name == "fetch").unwrap();
        assert_eq!(fetch.transport, McpTransport::Stdio);
        assert_eq!(fetch.command.as_deref(), Some("uvx"));
        assert_eq!(fetch.args, vec!["mcp-server-fetch".to_string()]);
        assert_eq!(fetch.env.get("LOG").map(String::as_str), Some("info"));
        let ctx = servers.iter().find(|s| s.name == "ctx").unwrap();
        assert_eq!(ctx.transport, McpTransport::Http);
        assert_eq!(ctx.url.as_deref(), Some("https://ctx.example.com/mcp"));
        // `headers` 应映射进 http_headers；`disabled:true` 等价 enabled=false。
        assert_eq!(ctx.http_headers.get("X-Key").map(String::as_str), Some("v"));
        assert!(!ctx.enabled);
    }

    #[test]
    fn parse_mcp_import_bare_map_and_toml() {
        // 直接「名称 -> 配置」映射（无 mcpServers 包裹）。
        let bare = r#"{ "memory": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"] } }"#;
        let servers = parse_mcp_import(bare).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "memory");

        // Codex TOML 片段。
        let toml = r#"
[mcp_servers.fetch]
command = "uvx"
args = ["mcp-server-fetch"]

[mcp_servers.relay]
url = "https://r.example.com/mcp"
"#;
        let mut servers = parse_mcp_import(toml).unwrap();
        servers.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "fetch");
        assert_eq!(servers[1].transport, McpTransport::Http);
    }

    #[test]
    fn parse_mcp_import_rejects_garbage_and_unnamed() {
        assert!(parse_mcp_import("").is_err(), "空内容应报错");
        assert!(parse_mcp_import("not json or toml @@@").is_err());
        // 单个 server 缺名称应报错（提示用 mcpServers 包裹）。
        assert!(parse_mcp_import(r#"{ "command": "uvx", "args": [] }"#).is_err());
    }

    // ---- Codex Profiles（独立文件）----

    #[test]
    fn profile_upsert_list_read_delete_roundtrip() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        assert!(cfg.list_profiles().unwrap().is_empty());

        cfg.upsert_profile(
            "deep-review",
            &CodexPreferences {
                model: Some("gpt-5.1-codex".into()),
                model_reasoning_effort: Some("xhigh".into()),
                ..Default::default()
            },
        )
        .unwrap();
        cfg.upsert_profile(
            "fast",
            &CodexPreferences {
                model: Some("gpt-5".into()),
                model_reasoning_effort: Some("low".into()),
                ..Default::default()
            },
        )
        .unwrap();

        // 写出的是独立文件 <name>.config.toml，内容为顶层键。
        let raw = fs::read_to_string(home.join("deep-review.config.toml")).unwrap();
        assert!(raw.contains("model = \"gpt-5.1-codex\""));
        assert!(raw.contains("model_reasoning_effort = \"xhigh\""));

        let list = cfg.list_profiles().unwrap();
        assert_eq!(list.len(), 2, "应列出两个 profile");
        assert_eq!(list[0].name, "deep-review", "按名排序");
        assert_eq!(list[1].name, "fast");

        let one = cfg.read_profile("fast").unwrap();
        assert_eq!(one.prefs.model.as_deref(), Some("gpt-5"));
        assert_eq!(one.prefs.model_reasoning_effort.as_deref(), Some("low"));

        // base config.toml 不应被 profile 影响。
        assert!(!cfg.config_path().exists() || !cfg.read_raw().unwrap().contains("gpt-5.1-codex"));

        assert!(cfg.delete_profile("fast").unwrap());
        assert_eq!(cfg.list_profiles().unwrap().len(), 1);
        assert!(!cfg.delete_profile("nope").unwrap(), "删除不存在返回 false");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn profile_rejects_bad_name_and_ignores_base_config() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        // base config.toml 不应被当作 profile（后缀不匹配）。
        fs::write(cfg.config_path(), "model = \"x\"\n").unwrap();
        assert!(cfg.list_profiles().unwrap().is_empty(), "config.toml 不是 profile");
        // 非法名称应被拒绝。
        assert!(cfg
            .upsert_profile("bad name", &CodexPreferences::default())
            .is_err());
        fs::remove_dir_all(&home).ok();
    }

    // ---- AGENTS.md ----

    #[test]
    fn agents_write_read_and_clear() {
        let home = temp_home();
        let cfg = CodexConfig::with_home(&home);
        assert_eq!(cfg.read_agents().unwrap(), "", "缺省返回空串");

        cfg.write_agents("# 团队约定\n- 用中文回复\n").unwrap();
        assert!(cfg.agents_path().exists());
        assert!(cfg.read_agents().unwrap().contains("团队约定"));

        // 空白内容清除文件。
        cfg.write_agents("   ").unwrap();
        assert!(!cfg.agents_path().exists(), "空白内容应删除 AGENTS.md");
        fs::remove_dir_all(&home).ok();
    }
}
