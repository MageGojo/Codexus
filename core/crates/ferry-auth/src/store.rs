//! 凭据存储：默认采用 CLIProxyAPI / Codex CLI 同款的**本地文件**凭据
//! （`~/.codexferry/auth/*.json`，目录 0700 / 文件 0600），与开源生态一致、
//! **不会触发 macOS 钥匙串密码弹窗**。
//!
//! 如需更强保护，可设 `FERRY_AUTH_BACKEND=keychain` 改用系统 Keychain。
//! 注意：未用稳定 Developer ID 签名的二进制，每次访问钥匙串都会被 macOS
//! 反复要求输入登录密码（这正是默认走文件后端的原因）。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use keyring_core::Entry;
use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "CodexFerry";
/// 文件后端（默认）下保存供应商 API Key 的文件名（明文，0600 权限）。
const PROVIDER_KEYS_FILE: &str = "provider-keys.json";
/// 文件后端（默认）下保存第三方服务 Key（如 apizero）的文件名（明文，0600 权限）。
const SERVICE_KEYS_FILE: &str = "service-keys.json";
/// 账号元数据（名称 / 标签 / 备注，**不含密钥**）的旁车文件名。
const ACCOUNT_META_FILE: &str = "account-meta.json";

/// 登录方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// ChatGPT OAuth 登录。
    Chatgpt,
    /// API Key 登录。
    ApiKey,
}

/// OAuth 令牌集合（落盘）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// 一个已登录账号。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// 供应商标识（`codex` 或具体供应商 id，如 `deepseek`）。
    pub provider: String,
    /// 稳定唯一 id：用于文件名、账号键、按账号统计与元数据关联。
    ///
    /// API Key 账号无 email/account_id，创建时生成 `{provider}-{8hex}` 并随文件持久化，
    /// 从而**同一供应商可挂多个账号互不覆盖**。OAuth 账号该字段通常为空，由
    /// email/account_id 派生稳定键（向后兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub auth_mode: AuthMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<StoredTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Keychain account 名称。存在时，token/API key 不在索引文件里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
}

impl Account {
    /// 用于展示的名称（email 优先，其次 account_id）。
    pub fn display_name(&self) -> String {
        self.email
            .clone()
            .or_else(|| self.account_id.clone())
            .unwrap_or_else(|| "未知账号".to_string())
    }

    /// 稳定唯一键：`id` > `secret_ref` > `email` > `account_id` > `{provider}-account`。
    ///
    /// 用作账号列表的 id、按账号统计的键、元数据关联键，确保同供应商多账号互不冲突，
    /// 同时对既有（无 `id` 的）账号保持与历史一致的键（向后兼容）。
    pub fn stable_id(&self) -> String {
        self.id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.secret_ref.clone())
            .or_else(|| self.email.clone())
            .or_else(|| self.account_id.clone())
            .unwrap_or_else(|| format!("{}-account", self.provider))
    }

    /// 导出为 CLIProxyAPI 同款 Codex OAuth 凭据文件结构。
    pub fn to_codex_auth_json(&self) -> serde_json::Value {
        let tokens = self.tokens.as_ref();
        serde_json::json!({
            "id_token": tokens.map(|t| t.id_token.as_str()).unwrap_or_default(),
            "access_token": tokens.map(|t| t.access_token.as_str()).unwrap_or_default(),
            "refresh_token": tokens.map(|t| t.refresh_token.as_str()).unwrap_or_default(),
            "account_id": self.account_id.as_deref().unwrap_or_default(),
            "last_refresh": self.last_refresh.map(|t| t.to_rfc3339()).unwrap_or_default(),
            "email": self.email.as_deref().unwrap_or_default(),
            "type": self.provider,
            "expired": self.expires_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        })
    }

    /// 导出为 Codex CLI 官方 `auth.json` 结构（嵌套 `tokens{}`）。
    ///
    /// 用于「OAuth 直连官方」：把账号 token 写入 `~/.codex/auth.json`，让 Codex 直连
    /// OpenAI 官方上游、不经码渡代理。仅 OAuth 账号有意义（无 tokens 时各字段为空串）。
    pub fn to_codex_cli_auth_json(&self) -> serde_json::Value {
        let tokens = self.tokens.as_ref();
        serde_json::json!({
            "OPENAI_API_KEY": serde_json::Value::Null,
            "tokens": {
                "id_token": tokens.map(|t| t.id_token.as_str()).unwrap_or_default(),
                "access_token": tokens.map(|t| t.access_token.as_str()).unwrap_or_default(),
                "refresh_token": tokens.map(|t| t.refresh_token.as_str()).unwrap_or_default(),
                "account_id": self.account_id.as_deref().unwrap_or_default(),
            },
            "last_refresh": self.last_refresh.map(|t| t.to_rfc3339()).unwrap_or_default(),
        })
    }
}

/// 账号元数据（用户可编辑的非密钥信息）：自定义名称、标签、备注。
///
/// 与凭据解耦，存于旁车 `account-meta.json`，键为 [`Account::stable_id`]。
/// 这样 OAuth 账号（凭据以 Codex 兼容形态落盘、无法塞自定义字段）也能带标签/备注。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountMeta {
    /// 自定义展示名（非空时覆盖默认展示名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 标签。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// 备注。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 该账号偏好的模型（中转/国内厂商账号选定的真实模型名；用该账号接管时写入 Codex）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl AccountMeta {
    /// 规整：去空白、去空标签、去重。
    pub fn normalized(mut self) -> Self {
        self.label = self
            .label
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.note = self
            .note
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.model = self
            .model
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut seen = std::collections::HashSet::new();
        self.tags = self
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty() && seen.insert(t.clone()))
            .collect();
        self
    }

    /// 是否为空（无任何元数据，可省略存储）。
    pub fn is_empty(&self) -> bool {
        self.label.is_none()
            && self.note.is_none()
            && self.tags.is_empty()
            && self.model.is_none()
    }
}

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    #[serde(default, rename = "type")]
    provider: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    last_refresh: String,
    #[serde(default)]
    email: String,
    #[serde(default, rename = "expired")]
    expires_at: String,
}

impl CodexAuthFile {
    fn into_account(self) -> Account {
        Account {
            provider: if self.provider.is_empty() {
                "codex".to_string()
            } else {
                self.provider
            },
            id: None,
            email: non_empty(self.email),
            account_id: non_empty(self.account_id),
            auth_mode: AuthMode::Chatgpt,
            tokens: Some(StoredTokens {
                id_token: self.id_token,
                access_token: self.access_token,
                refresh_token: self.refresh_token,
            }),
            api_key: None,
            last_refresh: parse_time(&self.last_refresh),
            expires_at: parse_time(&self.expires_at),
            secret_ref: None,
        }
    }
}

/// Codex CLI 官方 `~/.codex/auth.json` 的嵌套格式（与上面的 CLIProxyAPI 扁平格式不同）。
///
/// 形如 `{ "OPENAI_API_KEY": null, "tokens": { "id_token", "access_token",
/// "refresh_token", "account_id" }, "last_refresh": "..." }`。
#[derive(Debug, Deserialize)]
struct CodexCliAuthFile {
    #[serde(default, rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    #[serde(default)]
    tokens: Option<CodexCliTokens>,
    #[serde(default)]
    last_refresh: String,
}

#[derive(Debug, Deserialize)]
struct CodexCliTokens {
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    account_id: String,
}

impl CodexCliAuthFile {
    fn into_account(self) -> Result<Account> {
        if let Some(tokens) = self
            .tokens
            .filter(|t| !t.id_token.is_empty() || !t.access_token.is_empty())
        {
            // OAuth（ChatGPT 登录）：优先 JWT 解析身份，回退 tokens.account_id。
            let claims = crate::jwt::parse_id_token(&tokens.id_token).unwrap_or_default();
            Ok(Account {
                provider: "codex".to_string(),
                id: None,
                email: claims.email,
                account_id: claims
                    .account_id
                    .or_else(|| non_empty(tokens.account_id.clone())),
                auth_mode: AuthMode::Chatgpt,
                tokens: Some(StoredTokens {
                    id_token: tokens.id_token,
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token,
                }),
                api_key: None,
                last_refresh: parse_time(&self.last_refresh),
                expires_at: None,
                secret_ref: None,
            })
        } else if let Some(key) = self.openai_api_key.filter(|k| !k.trim().is_empty()) {
            Ok(Account {
                provider: "codex".to_string(),
                id: None,
                email: None,
                account_id: None,
                auth_mode: AuthMode::ApiKey,
                tokens: None,
                api_key: Some(key),
                last_refresh: parse_time(&self.last_refresh),
                expires_at: None,
                secret_ref: None,
            })
        } else {
            anyhow::bail!("auth.json 既无有效 tokens 也无 OPENAI_API_KEY")
        }
    }
}

/// 解析 Codex 凭据文本：优先官方嵌套格式（`~/.codex/auth.json`），
/// 回退 CLIProxyAPI 扁平格式或码渡原生 `Account`。
///
/// 会拒绝「空凭据」结果（无 token 且无 API Key），避免把 `{}` 之类的垃圾建成账号。
pub fn parse_codex_cli_auth(raw: &str) -> Result<Account> {
    if let Ok(parsed) = serde_json::from_str::<CodexCliAuthFile>(raw) {
        // 嵌套分支在 into_account 中已要求 token / key 非空。
        if let Ok(account) = parsed.into_account() {
            return Ok(account);
        }
    }
    let account = parse_account(raw)?;
    if !account_has_credentials(&account) {
        anyhow::bail!("凭据为空：未发现 token 或 API Key");
    }
    Ok(account)
}

/// 账号是否含可用凭据（非空 access_token / id_token，或非空 API Key）。
fn account_has_credentials(a: &Account) -> bool {
    let has_tokens = a
        .tokens
        .as_ref()
        .is_some_and(|t| !t.access_token.is_empty() || !t.id_token.is_empty());
    let has_key = a.api_key.as_ref().is_some_and(|k| !k.trim().is_empty());
    has_tokens || has_key
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthBackend {
    Keychain,
    File,
}

/// 凭据目录句柄。
#[derive(Clone)]
pub struct AuthStore {
    dir: PathBuf,
    backend: AuthBackend,
}

impl AuthStore {
    /// 定位凭据目录：优先 `FERRY_AUTH_DIR`，否则 `~/.codexferry/auth`。
    pub fn locate() -> Result<Self> {
        let dir = match std::env::var_os("FERRY_AUTH_DIR") {
            Some(d) => PathBuf::from(d),
            None => crate::home_dir()?.join(".codexferry").join("auth"),
        };
        // 默认文件后端（与开源生态一致、无钥匙串弹窗）。
        // 仅当显式 `FERRY_AUTH_BACKEND=keychain` 时才启用系统 Keychain。
        let backend = match std::env::var("FERRY_AUTH_BACKEND") {
            Ok(v) if v.eq_ignore_ascii_case("keychain") => AuthBackend::Keychain,
            _ => AuthBackend::File,
        };
        Ok(Self { dir, backend })
    }

    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            backend: AuthBackend::File,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file_for(&self, acc: &Account) -> PathBuf {
        // 有稳定 id（API Key 账号生成的 `{provider}-{8hex}`）：直接用 id 命名，
        // 保证同供应商多账号各占一个文件、互不覆盖。
        if let Some(id) = acc.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return self.dir.join(format!("{}.json", sanitize(id)));
        }
        // 兼容老路径：OAuth / 旧账号按 email|account_id 派生（与历史文件名一致）。
        let id = acc
            .email
            .clone()
            .or_else(|| acc.account_id.clone())
            .unwrap_or_else(|| "account".to_string());
        self.dir
            .join(format!("{}-{}.json", acc.provider, sanitize(&id)))
    }

    fn secret_key_for(&self, acc: &Account) -> String {
        self.file_for(acc)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("codex-account")
            .to_string()
    }

    fn index_account(&self, acc: &Account, secret_ref: String) -> Account {
        let mut index = acc.clone();
        index.tokens = None;
        index.api_key = None;
        index.secret_ref = Some(secret_ref);
        index
    }

    /// 保存（原子写入）账号，返回文件路径。
    pub fn save(&self, acc: &Account) -> Result<PathBuf> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("创建凭据目录 {} 失败", self.dir.display()))?;
        harden_dir(&self.dir);
        let path = self.file_for(acc);
        let json = self.secret_payload(acc)?;

        if self.backend == AuthBackend::Keychain {
            let key = self.secret_key_for(acc);
            match write_keychain_secret(&key, &json) {
                Ok(()) => {
                    let index = self.index_account(acc, key);
                    let index_json = serde_json::to_string_pretty(&index)?;
                    atomic_write(&path, &index_json)?;
                    tracing::info!("已保存账号凭据到 Keychain，索引 -> {}", path.display());
                    return Ok(path);
                }
                Err(e) => {
                    tracing::warn!("Keychain 写入失败，回退到文件式凭据: {e}");
                }
            }
        }

        atomic_write(&path, &json)?;
        tracing::info!("已保存账号凭据 -> {}", path.display());
        Ok(path)
    }

    fn secret_payload(&self, acc: &Account) -> Result<String> {
        let json = if acc.auth_mode == AuthMode::Chatgpt {
            serde_json::to_string_pretty(&acc.to_codex_auth_json())?
        } else {
            serde_json::to_string_pretty(acc)?
        };
        Ok(json)
    }

    /// 列出全部账号。
    pub fn list(&self) -> Result<Vec<Account>> {
        let mut out = Vec::new();
        if !self.dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            // 跳过同目录下的非账号索引文件（文件后端的供应商/服务密钥表），
            // 否则会被误解析成「未知账号」。
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == PROVIDER_KEYS_FILE
                || name == SERVICE_KEYS_FILE
                || name == ACCOUNT_META_FILE
            {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(s) => match parse_account(&s) {
                    Ok(a) => out.push(a),
                    Err(e) => tracing::warn!("跳过无法解析的凭据 {}: {e}", path.display()),
                },
                Err(e) => tracing::warn!("读取凭据 {} 失败: {e}", path.display()),
            }
        }
        Ok(out)
    }

    /// 删除账号对应文件。
    pub fn delete(&self, acc: &Account) -> Result<()> {
        // 仅 Keychain 后端清理钥匙串条目；文件后端绝不触碰钥匙串，避免弹密码框
        //（切换后残留的旧条目无害，可在系统「钥匙串访问」里手动清理）。
        if self.backend == AuthBackend::Keychain {
            if let Some(key) = &acc.secret_ref {
                let _ = delete_keychain_secret(key);
            }
        }
        let path = self.file_for(acc);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("删除凭据 {} 失败", path.display()))?;
        }
        Ok(())
    }

    /// 若账号来自 Keychain 索引，读取完整密钥材料；文件式账号则原样返回。
    pub fn load_secret(&self, acc: &Account) -> Result<Account> {
        let Some(key) = &acc.secret_ref else {
            return Ok(acc.clone());
        };
        // 文件后端不读钥匙串（避免 macOS 反复弹密码框）。文件账号的密钥已内联在
        // 文件里、不会带 secret_ref；这里能走到只可能是切换后残留的旧 Keychain
        // 索引，按原样返回（刷新任务与账号池会自动跳过无密钥账号，提示用户重添）。
        if self.backend == AuthBackend::File {
            return Ok(acc.clone());
        }
        let secret = read_keychain_secret(key)?;
        let mut full = parse_account(&secret)?;
        full.secret_ref = Some(key.clone());
        Ok(full)
    }

    // ---- 供应商 API Key（与账号解耦，按供应商 id 存取）----

    fn provider_keys_path(&self) -> PathBuf {
        self.dir.join(PROVIDER_KEYS_FILE)
    }

    fn read_provider_keys_file(&self) -> Result<HashMap<String, String>> {
        let path = self.provider_keys_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("读取供应商密钥文件 {} 失败", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(HashMap::new());
        }
        serde_json::from_str(&raw).context("解析供应商密钥文件失败")
    }

    fn write_provider_keys_file(&self, map: &HashMap<String, String>) -> Result<()> {
        let json = serde_json::to_string_pretty(map)?;
        atomic_write(&self.provider_keys_path(), &json)
    }

    /// 保存某供应商的 API Key：默认写入明文索引文件（0600）；
    /// `FERRY_AUTH_BACKEND=keychain` 时进系统 Keychain。
    pub fn set_provider_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let id = provider_id.trim();
        if id.is_empty() {
            anyhow::bail!("供应商 id 不能为空");
        }
        if self.backend == AuthBackend::Keychain {
            match write_keychain_secret(&provider_secret_key(id), api_key) {
                Ok(()) => return Ok(()),
                Err(e) => tracing::warn!("Keychain 写入供应商密钥失败，回退文件: {e}"),
            }
        }
        let mut map = self.read_provider_keys_file()?;
        map.insert(id.to_string(), api_key.to_string());
        self.write_provider_keys_file(&map)
    }

    /// 读取某供应商的 API Key（不存在返回 `None`）。
    pub fn get_provider_key(&self, provider_id: &str) -> Result<Option<String>> {
        let id = provider_id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        if self.backend == AuthBackend::Keychain {
            if let Ok(v) = read_keychain_secret(&provider_secret_key(id)) {
                return Ok(Some(v));
            }
        }
        Ok(self.read_provider_keys_file()?.get(id).cloned())
    }

    /// 删除某供应商的 API Key（Keychain 与文件索引均尝试清理）。
    pub fn delete_provider_key(&self, provider_id: &str) -> Result<()> {
        let id = provider_id.trim();
        if id.is_empty() {
            return Ok(());
        }
        if self.backend == AuthBackend::Keychain {
            let _ = delete_keychain_secret(&provider_secret_key(id));
        }
        let mut map = self.read_provider_keys_file()?;
        if map.remove(id).is_some() {
            self.write_provider_keys_file(&map)?;
        }
        Ok(())
    }

    // ---- 第三方服务密钥（如 apizero，用于天气/诗词等生活化集成）----

    fn service_keys_path(&self) -> PathBuf {
        self.dir.join(SERVICE_KEYS_FILE)
    }

    fn read_service_keys_file(&self) -> Result<HashMap<String, String>> {
        let path = self.service_keys_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("读取服务密钥文件 {} 失败", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(HashMap::new());
        }
        serde_json::from_str(&raw).context("解析服务密钥文件失败")
    }

    fn write_service_keys_file(&self, map: &HashMap<String, String>) -> Result<()> {
        let json = serde_json::to_string_pretty(map)?;
        atomic_write(&self.service_keys_path(), &json)
    }

    /// 保存某第三方服务的 Key：默认写入明文索引文件（0600）；
    /// `FERRY_AUTH_BACKEND=keychain` 时进系统 Keychain。
    pub fn set_service_key(&self, name: &str, key: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("服务名不能为空");
        }
        if self.backend == AuthBackend::Keychain {
            match write_keychain_secret(&service_secret_key(name), key) {
                Ok(()) => return Ok(()),
                Err(e) => tracing::warn!("Keychain 写入服务密钥失败，回退文件: {e}"),
            }
        }
        let mut map = self.read_service_keys_file()?;
        map.insert(name.to_string(), key.to_string());
        self.write_service_keys_file(&map)
    }

    /// 读取某第三方服务的 Key（不存在返回 `None`）。
    pub fn get_service_key(&self, name: &str) -> Result<Option<String>> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        if self.backend == AuthBackend::Keychain {
            if let Ok(v) = read_keychain_secret(&service_secret_key(name)) {
                return Ok(Some(v));
            }
        }
        Ok(self.read_service_keys_file()?.get(name).cloned())
    }

    /// 删除某第三方服务的 Key。
    pub fn delete_service_key(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        if self.backend == AuthBackend::Keychain {
            let _ = delete_keychain_secret(&service_secret_key(name));
        }
        let mut map = self.read_service_keys_file()?;
        if map.remove(name).is_some() {
            self.write_service_keys_file(&map)?;
        }
        Ok(())
    }

    // ---- 账号元数据（名称 / 标签 / 备注，不含密钥；旁车文件，与后端无关）----

    fn account_meta_path(&self) -> PathBuf {
        self.dir.join(ACCOUNT_META_FILE)
    }

    /// 读取全部账号元数据（键为账号 stable_id）。文件不存在/损坏时返回空表。
    pub fn account_meta_all(&self) -> Result<HashMap<String, AccountMeta>> {
        let path = self.account_meta_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("读取账号元数据 {} 失败", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(HashMap::new());
        }
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    fn write_account_meta_all(&self, map: &HashMap<String, AccountMeta>) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("创建凭据目录 {} 失败", self.dir.display()))?;
        harden_dir(&self.dir);
        let json = serde_json::to_string_pretty(map)?;
        atomic_write(&self.account_meta_path(), &json)
    }

    /// 读取单个账号的元数据（无则返回默认空值）。
    pub fn get_account_meta(&self, account_id: &str) -> Result<AccountMeta> {
        Ok(self
            .account_meta_all()?
            .remove(account_id.trim())
            .unwrap_or_default())
    }

    /// 设置（或在为空时清除）某账号的元数据。
    pub fn set_account_meta(&self, account_id: &str, meta: AccountMeta) -> Result<()> {
        let id = account_id.trim();
        if id.is_empty() {
            anyhow::bail!("账号 id 不能为空");
        }
        let meta = meta.normalized();
        let mut map = self.account_meta_all()?;
        if meta.is_empty() {
            map.remove(id);
        } else {
            map.insert(id.to_string(), meta);
        }
        self.write_account_meta_all(&map)
    }

    /// 删除某账号的元数据（账号被删除时调用）。
    pub fn delete_account_meta(&self, account_id: &str) -> Result<()> {
        let id = account_id.trim();
        if id.is_empty() {
            return Ok(());
        }
        let mut map = self.account_meta_all()?;
        if map.remove(id).is_some() {
            self.write_account_meta_all(&map)?;
        }
        Ok(())
    }
}

/// 把任意字符串收敛为安全的文件名片段（字母/数字/`-._@` 保留，其余转 `_`）。
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '.' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 为 API Key 账号生成稳定唯一 id：`{provider}-{8hex}`（随凭据文件持久化）。
pub fn generate_account_id(provider: &str) -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let p = provider.trim();
    let p = if p.is_empty() { "codex" } else { p };
    format!("{p}-{hex}")
}

fn provider_secret_key(id: &str) -> String {
    format!("provider-{id}")
}

fn service_secret_key(name: &str) -> String {
    format!("service-{name}")
}

fn parse_account(s: &str) -> Result<Account> {
    match serde_json::from_str::<Account>(s) {
        Ok(account) => Ok(account),
        Err(_) => serde_json::from_str::<CodexAuthFile>(s)
            .map(CodexAuthFile::into_account)
            .context("解析 CLIProxyAPI Codex 凭据失败"),
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn write_keychain_secret(key: &str, secret: &str) -> Result<()> {
    keyring::use_native_store(false).context("初始化系统 Keychain 失败")?;
    let entry = Entry::new(KEYCHAIN_SERVICE, key).context("创建 Keychain 条目失败")?;
    entry.set_password(secret).context("写入 Keychain 失败")
}

fn read_keychain_secret(key: &str) -> Result<String> {
    keyring::use_native_store(false).context("初始化系统 Keychain 失败")?;
    let entry = Entry::new(KEYCHAIN_SERVICE, key).context("创建 Keychain 条目失败")?;
    entry.get_password().context("读取 Keychain 失败")
}

fn delete_keychain_secret(key: &str) -> Result<()> {
    keyring::use_native_store(false).context("初始化系统 Keychain 失败")?;
    let entry = Entry::new(KEYCHAIN_SERVICE, key).context("创建 Keychain 条目失败")?;
    entry.delete_credential().context("删除 Keychain 条目失败")
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let dir = path.parent().context("目标路径缺少父目录")?;
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("cred");
    let tmp = dir.join(format!(".{file_name}.tmp.{}", std::process::id()));
    {
        // 凭据可能以明文落盘（文件后端），用 0600 创建，杜绝同机其他用户读取。
        let mut f = create_private_file(&tmp)
            .with_context(|| format!("创建临时文件 {} 失败", tmp.display()))?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("原子替换 {} 失败", path.display()))?;
    Ok(())
}

/// 以「仅属主可读写」(0600) 创建文件；非 unix 平台退化为普通创建。
#[cfg(unix)]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    fs::File::create(path)
}

/// 把凭据目录权限收紧到 0700（仅属主）。best-effort，失败不阻断。
fn harden_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("ferry-auth-{}", uuid_like()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn uuid_like() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )
    }

    fn sample(email: &str) -> Account {
        Account {
            provider: "codex".to_string(),
            id: None,
            email: Some(email.to_string()),
            account_id: Some("acc-1".to_string()),
            auth_mode: AuthMode::Chatgpt,
            tokens: Some(StoredTokens {
                id_token: "id".into(),
                access_token: "ac".into(),
                refresh_token: "rf".into(),
            }),
            api_key: None,
            last_refresh: Some(Utc::now()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            secret_ref: None,
        }
    }

    #[test]
    fn save_list_delete_roundtrip() {
        let dir = temp_dir();
        let store = AuthStore::with_dir(&dir);
        let acc = sample("user@example.com");
        store.save(&acc).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].email.as_deref(), Some("user@example.com"));
        assert_eq!(listed[0].auth_mode, AuthMode::Chatgpt);

        store.delete(&acc).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
        fs::remove_dir_all(&dir).ok();
    }

    fn api_key_sample(provider: &str) -> Account {
        Account {
            provider: provider.to_string(),
            id: Some(generate_account_id(provider)),
            email: None,
            account_id: None,
            auth_mode: AuthMode::ApiKey,
            tokens: None,
            api_key: Some("sk-demo".to_string()),
            last_refresh: Some(Utc::now()),
            expires_at: None,
            secret_ref: None,
        }
    }

    #[test]
    fn multiple_api_key_accounts_under_same_provider_do_not_collide() {
        let dir = temp_dir();
        let store = AuthStore::with_dir(&dir);
        // 两个 deepseek API Key 账号（不同生成 id）应各占一个文件、互不覆盖。
        let a = api_key_sample("deepseek");
        let b = api_key_sample("deepseek");
        assert_ne!(a.id, b.id, "生成的 id 应不同");
        store.save(&a).unwrap();
        store.save(&b).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2, "同供应商两个账号都应保留（不覆盖）");
        // 各自的 stable_id 即生成的 id。
        let ids: Vec<String> = listed.iter().map(|x| x.stable_id()).collect();
        assert!(ids.contains(&a.stable_id()));
        assert!(ids.contains(&b.stable_id()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn account_meta_roundtrip_and_clear() {
        let dir = temp_dir();
        let store = AuthStore::with_dir(&dir);
        let id = "deepseek-abc123";
        assert!(store.get_account_meta(id).unwrap().is_empty());

        store
            .set_account_meta(
                id,
                AccountMeta {
                    label: Some("  工作号 ".to_string()),
                    tags: vec!["工作".into(), "工作".into(), "  ".into(), "高优".into()],
                    note: Some("主力 DeepSeek".to_string()),
                    model: Some("  deepseek-reasoner  ".to_string()),
                },
            )
            .unwrap();
        let meta = store.get_account_meta(id).unwrap();
        assert_eq!(meta.label.as_deref(), Some("工作号"), "应去空白");
        assert_eq!(meta.tags, vec!["工作", "高优"], "应去空、去重");
        assert_eq!(meta.note.as_deref(), Some("主力 DeepSeek"));
        assert_eq!(meta.model.as_deref(), Some("deepseek-reasoner"), "model 应去空白");

        // 账号列表不应把 account-meta.json 误当账号。
        store.save(&api_key_sample("deepseek")).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);

        // 清空元数据。
        store.set_account_meta(id, AccountMeta::default()).unwrap();
        assert!(store.get_account_meta(id).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn file_backend_inlines_secret_and_uses_private_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let store = AuthStore::with_dir(&dir);
        let acc = sample("perm@example.com");
        let path = store.save(&acc).unwrap();

        // 文件后端：密钥内联在文件、不带 secret_ref，load_secret 不触碰钥匙串。
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].secret_ref.is_none(), "文件后端不应产生 Keychain 索引");
        let full = store.load_secret(&listed[0]).unwrap();
        assert!(full.tokens.is_some(), "应能直接从文件取回密钥");

        // 明文落盘必须是 0600，目录 0700。
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "凭据文件应为 0600");
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "凭据目录应为 0700");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn codex_auth_json_shape() {
        let acc = sample("u@e.com");
        let v = acc.to_codex_auth_json();
        assert_eq!(v["access_token"], "ac");
        assert_eq!(v["account_id"], "acc-1");
        assert_eq!(v["type"], "codex");
        assert!(v.get("last_refresh").is_some());
    }

    #[test]
    fn provider_key_file_roundtrip() {
        let dir = temp_dir();
        let store = AuthStore::with_dir(&dir);
        assert!(store.get_provider_key("myvendor").unwrap().is_none());

        store.set_provider_key("myvendor", "sk-demo-123").unwrap();
        assert_eq!(
            store.get_provider_key("myvendor").unwrap().as_deref(),
            Some("sk-demo-123")
        );

        // 覆盖更新
        store.set_provider_key("myvendor", "sk-demo-456").unwrap();
        assert_eq!(
            store.get_provider_key("myvendor").unwrap().as_deref(),
            Some("sk-demo-456")
        );

        store.delete_provider_key("myvendor").unwrap();
        assert!(store.get_provider_key("myvendor").unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn service_key_file_roundtrip() {
        let dir = temp_dir();
        let store = AuthStore::with_dir(&dir);
        assert!(store.get_service_key("apizero").unwrap().is_none());

        store.set_service_key("apizero", "az-key-1").unwrap();
        assert_eq!(
            store.get_service_key("apizero").unwrap().as_deref(),
            Some("az-key-1")
        );

        store.set_service_key("apizero", "az-key-2").unwrap();
        assert_eq!(
            store.get_service_key("apizero").unwrap().as_deref(),
            Some("az-key-2")
        );

        store.delete_service_key("apizero").unwrap();
        assert!(store.get_service_key("apizero").unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keychain_index_does_not_contain_secrets() {
        let store = AuthStore::with_dir(temp_dir());
        let acc = sample("safe@example.com");
        let index = store.index_account(&acc, "codex-safe@example.com".to_string());
        let json = serde_json::to_string(&index).unwrap();

        assert!(json.contains("safe@example.com"));
        assert!(json.contains("secret_ref"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
        assert!(!json.contains("\"ac\""));
        assert!(!json.contains("\"rf\""));
    }

    #[test]
    fn parse_codex_cli_auth_reads_nested_oauth() {
        let raw = r#"{
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "not-a-real-jwt",
                "access_token": "acc-token",
                "refresh_token": "ref-token",
                "account_id": "acct-123"
            },
            "last_refresh": "2026-06-17T00:00:00Z"
        }"#;
        let account = parse_codex_cli_auth(raw).unwrap();
        assert_eq!(account.auth_mode, AuthMode::Chatgpt);
        assert_eq!(account.account_id.as_deref(), Some("acct-123"));
        let tokens = account.tokens.unwrap();
        assert_eq!(tokens.access_token, "acc-token");
        assert_eq!(tokens.refresh_token, "ref-token");
    }

    #[test]
    fn parse_codex_cli_auth_reads_api_key_form() {
        let raw = r#"{ "OPENAI_API_KEY": "sk-local-xyz", "tokens": null }"#;
        let account = parse_codex_cli_auth(raw).unwrap();
        assert_eq!(account.auth_mode, AuthMode::ApiKey);
        assert_eq!(account.api_key.as_deref(), Some("sk-local-xyz"));
    }

    #[test]
    fn parse_codex_cli_auth_falls_back_to_flat_format() {
        // CLIProxyAPI 扁平格式应仍可解析。
        let raw = r#"{ "type": "codex", "id_token": "id", "access_token": "ac",
            "refresh_token": "rf", "account_id": "flat-1", "email": "flat@example.com" }"#;
        let account = parse_codex_cli_auth(raw).unwrap();
        assert_eq!(account.auth_mode, AuthMode::Chatgpt);
        assert_eq!(account.email.as_deref(), Some("flat@example.com"));
        assert_eq!(account.account_id.as_deref(), Some("flat-1"));
    }
}
