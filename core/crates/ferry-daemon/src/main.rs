//! ferry-daemon：码渡核心 sidecar 入口（M0–M2 阶段）。
//!
//! 子命令（对齐 CLIProxyAPI 的登录接入方式）：
//! - `serve`（默认）           启动本地代理网关
//! - `codex-login [--no-browser]`  ChatGPT OAuth 登录（多账号：重复执行即可）
//! - `codex-login-key <KEY>`   API Key 登录
//! - `accounts`                列出已登录账号
//! - `providers`               列出内置供应商预设
//!
//! 代理相关环境变量见各处默认值。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Result;
use ferry_auth::{Account, AuthStore, OAuthConfig};
use ferry_config::{
    find_provider_preset, provider_presets, resolve_provider, CodexConfig, ProviderApi,
    ProviderEntry, ProviderStore, SettingsStore,
};
use ferry_convert::UpstreamApi;
use ferry_proxy::{AccountPool, AppState, PoolStrategy, ProviderConfig, RouteMode};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("serve");

    match cmd {
        "serve" => serve().await,
        "codex-login" => {
            let no_browser = args
                .iter()
                .any(|a| a == "--no-browser" || a == "-no-browser");
            codex_login(!no_browser).await
        }
        "codex-login-key" => {
            let key = args.get(1).cloned().unwrap_or_default();
            codex_login_key(key)
        }
        "accounts" => list_accounts(),
        "providers" => list_providers(),
        other => {
            eprintln!("未知命令: {other}\n可用: serve | providers | codex-login [--no-browser] | codex-login-key <KEY> | accounts");
            std::process::exit(2);
        }
    }
}

async fn serve() -> Result<()> {
    // 改名迁移：把旧数据目录 ~/.codeferry 整体改名为 ~/.codexferry。
    // 必须早于任何存储（账号/会话/设置/供应商/ipc_token）初始化。
    migrate_legacy_data_dir();
    let listen: SocketAddr = env_or("FERRY_LISTEN", "127.0.0.1:15721").parse()?;
    let ipc_listen: SocketAddr = env_or("FERRY_IPC_LISTEN", "127.0.0.1:15722").parse()?;
    let provider_id = env_or("FERRY_PROVIDER", "deepseek");
    let auth_store = AuthStore::locate().ok();
    let provider_store = ProviderStore::locate().ok();

    let entry = match provider_store.as_ref() {
        Some(store) => resolve_provider(store, &provider_id).ok().flatten(),
        None => find_provider_preset(&provider_id).map(ProviderEntry::from_preset),
    }
    .unwrap_or_else(|| {
        eprintln!("未知供应商: {provider_id}\n可用内置预设:");
        print_provider_list();
        std::process::exit(2);
    });
    let provider = build_provider(&entry, auth_store.as_ref());

    if provider.api_key.is_empty() {
        tracing::warn!(
            "API Key 为空：请在 GUI 设置、或设置 FERRY_API_KEY / 供应商环境变量 {:?}",
            entry.api_key_env
        );
    }

    let base_url = provider.base_url.clone();
    let default_model = provider.default_model.clone();
    let api_type = provider.api_type;
    let provider_state = Arc::new(RwLock::new(provider));

    // 路由模式：FERRY_ROUTE_MODE = provider（默认）| pool。
    let route_mode = std::env::var("FERRY_ROUTE_MODE")
        .ok()
        .and_then(|s| RouteMode::parse(&s))
        .unwrap_or(RouteMode::Provider);
    let mode_state = Arc::new(RwLock::new(route_mode));

    // 应用设置（天气城市、生活化开关、账号池调度策略等）。
    let settings_store = SettingsStore::locate().ok();
    let settings = settings_store
        .as_ref()
        .map(|s| s.load())
        .unwrap_or_default();

    // Codex 账号池：从账号存储装配（仅含 access_token 的 ChatGPT 账号）。
    let pool_accounts = auth_store
        .as_ref()
        .map(ferry_ipc::load_pool_accounts)
        .unwrap_or_default();
    let pool_count = pool_accounts.len();
    let mut pool = AccountPool::new(pool_accounts);
    if settings.pool_quota_aware {
        pool.set_strategy(PoolStrategy::QuotaAware);
    }
    let pool_state = Arc::new(RwLock::new(pool));

    // 管理 API 本地鉴权 token（GUI 与 daemon 共享）。
    let ipc_token = load_or_create_ipc_token();
    let http_client = reqwest::Client::new();

    let session_store = open_store();
    let state = AppState {
        provider: provider_state.clone(),
        pool: pool_state.clone(),
        mode: mode_state.clone(),
        http: http_client.clone(),
        store: session_store.clone(),
    };

    let ipc_state = ferry_ipc::IpcState {
        auth_store: auth_store.clone(),
        session_store,
        codex_config: CodexConfig::locate().ok(),
        active_provider: Some(provider_state),
        provider_store,
        account_pool: Some(pool_state.clone()),
        route_mode: Some(mode_state),
        settings_store,
        auth_token: ipc_token,
        http_client,
        weather_cache: Default::default(),
    };
    tokio::spawn(async move {
        if let Err(e) = ferry_ipc::serve(ipc_listen, ipc_state).await {
            tracing::warn!("ferry-ipc 已退出: {e}");
        }
    });

    // 后台：定期刷新临近过期的账号 token 并回灌账号池，保证账号池长期可用。
    if let Some(auth) = auth_store {
        spawn_token_refresher(auth, pool_state);
    }

    tracing::info!(
        "Codexus daemon：模式 {} / 供应商上游 {base_url} 默认模型 {default_model} ({api_type:?}) / 账号池 {pool_count} 个 Codex 账号",
        route_mode.as_str()
    );
    ferry_proxy::serve(listen, state).await
}

/// 后台 token 刷新任务：周期性检查账号池中临近过期的 OAuth token 并续期。
fn spawn_token_refresher(auth: AuthStore, pool: Arc<RwLock<AccountPool>>) {
    let interval = Duration::from_secs(
        std::env::var("FERRY_TOKEN_REFRESH_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&s| s >= 30)
            .unwrap_or(300),
    );
    tokio::spawn(async move {
        let config = OAuthConfig::default();
        loop {
            tokio::time::sleep(interval).await;
            refresh_pool_tokens(&auth, &pool, &config).await;
        }
    });
}

/// 刷新所有临近过期（剩余 < 5 分钟）的 OAuth 账号 token，并更新账号池。
async fn refresh_pool_tokens(auth: &AuthStore, pool: &Arc<RwLock<AccountPool>>, config: &OAuthConfig) {
    let accounts = match auth.list() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("刷新任务读取账号失败: {e}");
            return;
        }
    };
    let soon = chrono::Utc::now() + chrono::Duration::minutes(5);
    for acc in accounts {
        let mut full = match auth.load_secret(&acc) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let needs_refresh = full.expires_at.is_some_and(|e| e <= soon);
        let has_refresh = full
            .tokens
            .as_ref()
            .is_some_and(|t| !t.refresh_token.trim().is_empty());
        if !needs_refresh || !has_refresh {
            continue;
        }
        match ferry_auth::refresh_account(config, auth, &mut full).await {
            Ok(()) => {
                let key = pool_account_key(&full);
                if let (Some(tokens), Ok(mut guard)) = (full.tokens.as_ref(), pool.write()) {
                    guard.update_token(
                        &key,
                        tokens.access_token.clone(),
                        full.expires_at.map(|e| e.timestamp()),
                    );
                }
                tracing::info!("已刷新账号 token：{}", full.display_name());
            }
            Err(e) => tracing::warn!("刷新账号 {} token 失败: {e}", full.display_name()),
        }
    }
}

/// 账号池键：与 `ferry_ipc::load_pool_accounts` 的键生成保持一致（账号稳定 id）。
fn pool_account_key(a: &Account) -> String {
    a.stable_id()
}

fn build_provider(entry: &ProviderEntry, auth_store: Option<&AuthStore>) -> ProviderConfig {
    let base_url = std::env::var("FERRY_BASE_URL").unwrap_or_else(|_| entry.base_url.clone());
    let default_model =
        std::env::var("FERRY_MODEL").unwrap_or_else(|_| entry.default_model.clone());
    // Key 优先级：FERRY_API_KEY > 已保存(Keychain/文件) > 供应商环境变量。
    let api_key = std::env::var("FERRY_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| auth_store.and_then(|s| s.get_provider_key(&entry.id).ok().flatten()))
        .or_else(|| entry.api_key_from_env())
        .unwrap_or_default();
    let api_type = match std::env::var("FERRY_API_TYPE").ok().as_deref() {
        Some("responses") => UpstreamApi::Responses,
        Some("chat") | None => provider_api_to_upstream(entry.api),
        Some(other) => {
            tracing::warn!("未知 FERRY_API_TYPE={other}，回退到供应商预设");
            provider_api_to_upstream(entry.api)
        }
    };

    let model_map = entry
        .aliases
        .iter()
        .map(|alias| {
            let target = if alias.to == entry.default_model {
                default_model.clone()
            } else {
                alias.to.clone()
            };
            (alias.from.clone(), target)
        })
        .collect::<HashMap<_, _>>();

    ProviderConfig {
        base_url,
        api_key,
        api_type,
        default_model,
        model_map,
        // 启动默认供应商时未绑定具体账号；运行时由 GUI「使用账号」切换时带上账号 id。
        account_key: String::new(),
    }
}

fn provider_api_to_upstream(api: ProviderApi) -> UpstreamApi {
    match api {
        ProviderApi::Chat => UpstreamApi::Chat,
        ProviderApi::Responses => UpstreamApi::Responses,
    }
}

async fn codex_login(open_browser: bool) -> Result<()> {
    let store = AuthStore::locate()?;
    let config = OAuthConfig::default();
    println!(
        "开始 ChatGPT OAuth 登录（回调端口 {}）…",
        config.callback_port
    );
    let account = ferry_auth::login_with_browser(&config, &store, open_browser).await?;
    println!(
        "✅ 登录成功：{}（凭据目录 {}）",
        account.display_name(),
        store.dir().display()
    );
    Ok(())
}

fn codex_login_key(key: String) -> Result<()> {
    if key.is_empty() {
        anyhow::bail!("请提供 API Key：ferry-daemon codex-login-key <KEY>");
    }
    let store = AuthStore::locate()?;
    let account = ferry_auth::login_with_api_key(&store, key)?;
    println!(
        "✅ 已保存 API Key 账号（凭据目录 {}）",
        store.dir().display()
    );
    let _ = account;
    Ok(())
}

fn list_accounts() -> Result<()> {
    let store = AuthStore::locate()?;
    let accounts = store.list()?;
    if accounts.is_empty() {
        println!("（暂无已登录账号，凭据目录 {}）", store.dir().display());
        return Ok(());
    }
    println!("已登录账号（{}）：", accounts.len());
    for a in accounts {
        println!(
            "  - [{}] {} ({:?})",
            a.provider,
            a.display_name(),
            a.auth_mode
        );
    }
    Ok(())
}

fn list_providers() -> Result<()> {
    print_provider_list();
    Ok(())
}

fn print_provider_list() {
    println!("内置供应商预设：");
    for p in provider_presets() {
        println!(
            "  - {:<8} {:<18} {}  model={}  env={}",
            p.id,
            p.name,
            p.base_url,
            p.default_model,
            p.api_key_env.join("|")
        );
    }
}

fn open_store() -> Option<Arc<Mutex<ferry_store::SessionStore>>> {
    match ferry_store::SessionStore::locate() {
        Ok(store) => Some(Arc::new(Mutex::new(store))),
        Err(e) => {
            tracing::warn!("会话存储不可用，代理仍会继续运行: {e}");
            None
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 启动时一次性数据目录迁移（改名 CodeFerry -> CodexFerry 的配套）：
/// 把旧目录 `~/.codeferry` 整体改名为 `~/.codexferry`，保留全部历史数据
/// （账号、会话库、设置、供应商、ipc_token）。仅当新目录不存在且旧目录存在
/// 时执行，幂等、无数据丢失；失败也不阻塞启动（退化为在新目录重新开始）。
/// 跨平台用户主目录：`HOME`（类 Unix）或 `USERPROFILE`（Windows）。
fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

fn migrate_legacy_data_dir() {
    let Some(home) = home_dir() else {
        return;
    };
    let old = home.join(".codeferry");
    let new = home.join(".codexferry");
    if new.exists() || !old.exists() {
        return;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => tracing::info!(
            "已迁移数据目录 {} -> {}",
            old.display(),
            new.display()
        ),
        Err(e) => tracing::warn!(
            "迁移数据目录失败（{} -> {}）: {e}；将使用新目录重新开始",
            old.display(),
            new.display()
        ),
    }
}

/// 解析 `~/.codexferry/ipc_token` 路径（用于 GUI 与 daemon 共享本地鉴权 token）。
fn ipc_token_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("FERRY_IPC_TOKEN_FILE") {
        return Some(std::path::PathBuf::from(p));
    }
    home_dir().map(|home| home.join(".codexferry").join("ipc_token"))
}

/// 读取或生成管理 API 本地鉴权 token：
/// 优先 `FERRY_IPC_TOKEN` 环境变量；否则读 `~/.codexferry/ipc_token`；
/// 不存在则生成 256 位随机 token 并以 0600 权限落盘供 GUI 读取。
fn load_or_create_ipc_token() -> Option<String> {
    if let Ok(t) = std::env::var("FERRY_IPC_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    let path = ipc_token_path()?;
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_string();
        if !existing.is_empty() {
            tracing::info!("已加载管理 API token：{}", path.display());
            return Some(existing);
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    if let Err(e) = write_token_file(&path, &token) {
        tracing::warn!("写入 IPC token 文件失败（鉴权仍生效，但 GUI 可能需手动配置）: {e}");
    } else {
        tracing::info!("已生成管理 API token：{}", path.display());
    }
    Some(token)
}

fn write_token_file(path: &std::path::Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, token)
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("ferry_daemon=info,ferry_proxy=info,ferry_auth=info")
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
