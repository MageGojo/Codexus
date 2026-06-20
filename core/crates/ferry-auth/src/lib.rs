//! ferry-auth：Codex 登录（CLIProxyAPI 同款接入方式）。
//!
//! - **ChatGPT OAuth 2.0 + PKCE**：复刻 Codex CLI 的浏览器登录流程
//!   （`client_id`、`/oauth/authorize`、本地 `1455/auth/callback`、`/oauth/token`）。
//! - **API Key 登录**：直接保存 `sk-...`。
//! - **多账号**：每账号一个 JSON 文件，便于轮询与切换（见 [`AuthStore`]）。
//! - **令牌刷新**：用 refresh_token 续期（见 [`refresh_account`]）。
//!
//! 高层入口：[`login_with_browser`]、[`login_with_api_key`]、[`refresh_account`]。

mod callback;
mod jwt;
mod oauth;
mod pkce;
mod store;

pub use callback::wait_for_code;
pub use jwt::{parse_id_token, IdTokenClaims};
pub use oauth::{
    begin, build_authorize_url, exchange_code, refresh, OAuthConfig, PendingAuth, TokenSet,
};
pub use pkce::{generate_pkce, PkceCodes};
pub use store::{
    generate_account_id, parse_codex_cli_auth, Account, AccountMeta, AuthMode, AuthStore,
    StoredTokens,
};

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("无法确定用户主目录（HOME / USERPROFILE 均未设置）")
}

/// 浏览器 OAuth 登录全流程：发起授权 -> 等待回调 -> 交换令牌 -> 解析身份 -> 保存账号。
///
/// `open_browser` 为 true 时自动打开系统浏览器（macOS `open`），否则仅打印链接
/// （对应 CLIProxyAPI 的 `-no-browser`）。
pub async fn login_with_browser(
    config: &OAuthConfig,
    store: &AuthStore,
    open_browser: bool,
) -> Result<Account> {
    let pending = begin(config);

    println!(
        "\n请在浏览器中完成 ChatGPT 登录授权：\n{}\n",
        pending.auth_url
    );
    if open_browser {
        if let Err(e) = open_url(&pending.auth_url) {
            tracing::warn!("自动打开浏览器失败（请手动复制上面的链接）: {e}");
        }
    }

    let code = callback::wait_for_code(
        config.callback_port,
        pending.state.clone(),
        Duration::from_secs(300),
    )
    .await?;

    let tokens = exchange_code(config, &pending.pkce, &pending.redirect_uri, &code).await?;
    let claims = parse_id_token(&tokens.id_token).unwrap_or_default();
    let expires_at = expires_at(tokens.expires_in);

    let account = Account {
        provider: "codex".to_string(),
        id: None,
        email: claims.email,
        account_id: claims.account_id,
        auth_mode: AuthMode::Chatgpt,
        tokens: Some(StoredTokens {
            id_token: tokens.id_token,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
        }),
        api_key: None,
        last_refresh: Some(chrono::Utc::now()),
        expires_at,
        secret_ref: None,
    };
    store.save(&account)?;
    Ok(account)
}

/// API Key 登录：直接保存账号（通用 `codex` 供应商）。
pub fn login_with_api_key(store: &AuthStore, api_key: impl Into<String>) -> Result<Account> {
    login_with_api_key_for(store, api_key, "codex")
}

/// API Key 登录并归属到指定供应商：账号 `provider` 记为该供应商 id，
/// 便于在账号页区分展示（不同供应商落不同文件，互不覆盖）。
///
/// 注意：这里只负责建账号；若还要让代理在启用该供应商时取用该 Key，
/// 调用方需另行 [`AuthStore::set_provider_key`]。
pub fn login_with_api_key_for(
    store: &AuthStore,
    api_key: impl Into<String>,
    provider: impl Into<String>,
) -> Result<Account> {
    let provider = provider.into();
    let provider = if provider.trim().is_empty() {
        "codex".to_string()
    } else {
        provider
    };
    // 生成稳定唯一 id：同供应商可挂多个 API Key 账号，互不覆盖。
    let id = generate_account_id(&provider);
    let account = Account {
        provider,
        id: Some(id),
        email: None,
        account_id: None,
        auth_mode: AuthMode::ApiKey,
        tokens: None,
        api_key: Some(api_key.into()),
        last_refresh: Some(chrono::Utc::now()),
        expires_at: None,
        secret_ref: None,
    };
    store.save(&account)?;
    Ok(account)
}

/// Token 三件套登录：由粘贴的 id_token / access_token /（可选）refresh_token 直接建号。
///
/// 会尝试从 id_token（JWT）解析 email / account_id；解析失败不阻断建号。
pub fn login_with_tokens(
    store: &AuthStore,
    id_token: impl Into<String>,
    access_token: impl Into<String>,
    refresh_token: Option<String>,
) -> Result<Account> {
    let id_token = id_token.into();
    let claims = parse_id_token(&id_token).unwrap_or_default();
    let account = Account {
        provider: "codex".to_string(),
        id: None,
        email: claims.email,
        account_id: claims.account_id,
        auth_mode: AuthMode::Chatgpt,
        tokens: Some(StoredTokens {
            id_token,
            access_token: access_token.into(),
            refresh_token: refresh_token.unwrap_or_default(),
        }),
        api_key: None,
        last_refresh: Some(chrono::Utc::now()),
        expires_at: None,
        secret_ref: None,
    };
    store.save(&account)?;
    Ok(account)
}

/// 从本机 Codex 凭据文件（如 `~/.codex/auth.json`）导入账号并入库。
///
/// 同时支持官方嵌套格式与 CLIProxyAPI 扁平格式（见 [`parse_codex_cli_auth`]）。
pub fn import_codex_account(store: &AuthStore, auth_json_path: &Path) -> Result<Account> {
    let raw = std::fs::read_to_string(auth_json_path)
        .with_context(|| format!("读取 {} 失败", auth_json_path.display()))?;
    let account = parse_codex_cli_auth(&raw)?;
    store.save(&account)?;
    Ok(account)
}

/// 从粘贴的 JSON 文本导入账号（程序运行时**自动识别格式**）：
///
/// 接受单个账号对象、账号数组，或 `{ "accounts": [...] }` 包裹；每个元素再交由
/// [`parse_codex_cli_auth`] 自动识别（官方嵌套 / CLIProxyAPI 扁平 / 裸 token 对象 /
/// 原生 `Account`）。返回成功入库的账号列表；全部失败则报错。
pub fn import_codex_from_json(store: &AuthStore, content: &str) -> Result<Vec<Account>> {
    let value: serde_json::Value = serde_json::from_str(content.trim())
        .context("内容不是合法 JSON")?;
    let items: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr,
        serde_json::Value::Object(ref map) => match map.get("accounts") {
            Some(serde_json::Value::Array(arr)) => arr.clone(),
            _ => vec![value.clone()],
        },
        other => vec![other],
    };

    let mut saved = Vec::new();
    let mut errors = Vec::new();
    for item in items {
        match parse_codex_cli_auth(&item.to_string()) {
            Ok(mut account) => {
                // 无身份（API Key 且无 email/account_id）的账号生成唯一 id，
                // 避免一次导入多个无身份账号时互相覆盖。
                if account.id.is_none()
                    && account.email.is_none()
                    && account.account_id.is_none()
                {
                    account.id = Some(generate_account_id(&account.provider));
                }
                store.save(&account)?;
                saved.push(account);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }

    if saved.is_empty() {
        if errors.is_empty() {
            anyhow::bail!("未识别到有效账号");
        }
        anyhow::bail!("未识别到有效账号：{}", errors.join("；"));
    }
    Ok(saved)
}

/// 刷新账号令牌并按 CLIProxyAPI 同款结构落盘。
pub async fn refresh_account(
    config: &OAuthConfig,
    store: &AuthStore,
    account: &mut Account,
) -> Result<()> {
    let Some(tokens) = &account.tokens else {
        return Ok(());
    };
    let new = refresh(config, &tokens.refresh_token).await?;
    account.tokens = Some(StoredTokens {
        id_token: new.id_token,
        access_token: new.access_token,
        refresh_token: new.refresh_token,
    });
    account.last_refresh = Some(chrono::Utc::now());
    account.expires_at = expires_at(new.expires_in);
    store.save(account)?;
    Ok(())
}

fn expires_at(expires_in: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    if expires_in <= 0 {
        return None;
    }
    Some(chrono::Utc::now() + chrono::Duration::seconds(expires_in))
}

/// 打开系统默认浏览器：macOS `open` / Windows `cmd /C start` / Linux `xdg-open`。
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        // `start` 是 cmd 内建命令；第一个空串是窗口标题占位，防止 URL 被当成标题。
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "ferry-auth-lib-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_jwt(email: &str, account_id: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload = serde_json::json!({ "email": email, "chatgpt_account_id": account_id });
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{body}.sig")
    }

    #[test]
    fn login_with_tokens_parses_identity_and_saves() {
        let dir = temp_dir("tokens");
        let store = AuthStore::with_dir(&dir);
        let jwt = make_jwt("dev@example.com", "acc-xyz");
        let account =
            login_with_tokens(&store, jwt, "access-abc", Some("refresh-def".to_string())).unwrap();

        assert_eq!(account.email.as_deref(), Some("dev@example.com"));
        assert_eq!(account.account_id.as_deref(), Some("acc-xyz"));
        assert_eq!(account.auth_mode, AuthMode::Chatgpt);

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].email.as_deref(), Some("dev@example.com"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_codex_account_reads_nested_oauth() {
        let dir = temp_dir("imp-oauth");
        let store = AuthStore::with_dir(&dir);
        let codex_home = temp_dir("imp-oauth-home");
        let auth_path = codex_home.join("auth.json");
        let jwt = make_jwt("imported@example.com", "acc-imp");
        let body = serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt,
                "access_token": "imp-access",
                "refresh_token": "imp-refresh",
                "account_id": "acc-imp"
            },
            "last_refresh": "2026-06-17T00:00:00Z"
        });
        fs::write(&auth_path, serde_json::to_string_pretty(&body).unwrap()).unwrap();

        let account = import_codex_account(&store, &auth_path).unwrap();
        assert_eq!(account.email.as_deref(), Some("imported@example.com"));
        assert_eq!(account.auth_mode, AuthMode::Chatgpt);
        assert_eq!(store.list().unwrap().len(), 1);

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&codex_home).ok();
    }

    #[test]
    fn import_codex_account_reads_api_key_form() {
        let dir = temp_dir("imp-key");
        let store = AuthStore::with_dir(&dir);
        let codex_home = temp_dir("imp-key-home");
        let auth_path = codex_home.join("auth.json");
        fs::write(
            &auth_path,
            r#"{ "OPENAI_API_KEY": "sk-imported-123", "tokens": null }"#,
        )
        .unwrap();

        let account = import_codex_account(&store, &auth_path).unwrap();
        assert_eq!(account.auth_mode, AuthMode::ApiKey);
        assert_eq!(account.api_key.as_deref(), Some("sk-imported-123"));

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&codex_home).ok();
    }

    #[test]
    fn import_codex_account_missing_file_errors() {
        let dir = temp_dir("imp-missing");
        let store = AuthStore::with_dir(&dir);
        let missing = dir.join("nope").join("auth.json");
        assert!(import_codex_account(&store, &missing).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_from_json_single_object_auto_detected() {
        let dir = temp_dir("json-one");
        let store = AuthStore::with_dir(&dir);
        let content = r#"{
            "tokens": {
                "id_token": "x", "access_token": "a",
                "refresh_token": "r", "account_id": "json-acct"
            }
        }"#;
        let saved = import_codex_from_json(&store, content).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].account_id.as_deref(), Some("json-acct"));
        assert_eq!(store.list().unwrap().len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_from_json_array_and_wrapper() {
        let dir = temp_dir("json-arr");
        let store = AuthStore::with_dir(&dir);
        // 数组：两个不同身份（扁平格式）。
        let arr = r#"[
            { "type": "codex", "access_token": "a1", "email": "a@x.com", "account_id": "acc-a" },
            { "type": "codex", "access_token": "a2", "email": "b@x.com", "account_id": "acc-b" }
        ]"#;
        let saved = import_codex_from_json(&store, arr).unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(store.list().unwrap().len(), 2);

        // {accounts:[...]} 包裹。
        let wrapped =
            r#"{ "accounts": [ { "type": "codex", "access_token": "a3", "email": "c@x.com" } ] }"#;
        let saved = import_codex_from_json(&store, wrapped).unwrap();
        assert_eq!(saved.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_from_json_invalid_errors() {
        let dir = temp_dir("json-bad");
        let store = AuthStore::with_dir(&dir);
        assert!(import_codex_from_json(&store, "not json").is_err());
        assert!(import_codex_from_json(&store, "{}").is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
