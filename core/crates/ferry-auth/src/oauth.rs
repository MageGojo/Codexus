//! Codex OAuth 2.0 + PKCE 流程（CLIProxyAPI / Codex CLI 同款参数）。

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::pkce::{generate_pkce, random_urlsafe, PkceCodes};

/// OAuth 配置（默认值与 Codex CLI / CLIProxyAPI 一致）。
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub callback_port: u16,
    pub scope: String,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            issuer: "https://auth.openai.com".to_string(),
            // Codex CLI 公开 client_id（CLIProxyAPI 同款）
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_string(),
            callback_port: 1455,
            scope: "openid email profile offline_access".to_string(),
        }
    }
}

impl OAuthConfig {
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/auth/callback", self.callback_port)
    }

    fn token_endpoint(&self) -> String {
        format!("{}/oauth/token", self.issuer.trim_end_matches('/'))
    }
}

/// 一次待完成的授权（含授权 URL、state、PKCE）。
#[derive(Debug, Clone)]
pub struct PendingAuth {
    pub auth_url: String,
    pub state: String,
    pub pkce: PkceCodes,
    pub redirect_uri: String,
}

/// 发起授权：生成 PKCE/state 与授权 URL。
pub fn begin(config: &OAuthConfig) -> PendingAuth {
    let pkce = generate_pkce();
    let state = random_urlsafe(24);
    let redirect_uri = config.redirect_uri();
    let auth_url = build_authorize_url(config, &redirect_uri, &pkce, &state);
    PendingAuth {
        auth_url,
        state,
        pkce,
        redirect_uri,
    }
}

/// 构造 `/oauth/authorize` URL。
pub fn build_authorize_url(
    config: &OAuthConfig,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let client_id = config.client_id.as_str();
    let scope = config.scope.as_str();
    let challenge = pkce.code_challenge.as_str();
    let params: [(&str, &str); 10] = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", scope),
        ("prompt", "login"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
    ];
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!(
        "{}/oauth/authorize?{qs}",
        config.issuer.trim_end_matches('/')
    )
}

/// token 端点返回的令牌集合。
#[derive(Debug, Clone, Deserialize)]
pub struct TokenSet {
    pub id_token: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: i64,
}

/// 用授权码交换令牌。
pub async fn exchange_code(
    config: &OAuthConfig,
    pkce: &PkceCodes,
    redirect_uri: &str,
    code: &str,
) -> Result<TokenSet> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencode(code),
        urlencode(redirect_uri),
        urlencode(&config.client_id),
        urlencode(&pkce.code_verifier),
    );
    post_token(config, body).await.context("交换授权码失败")
}

/// 用 refresh_token 刷新令牌。
pub async fn refresh(config: &OAuthConfig, refresh_token: &str) -> Result<TokenSet> {
    let body = format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}&scope={}",
        urlencode(&config.client_id),
        urlencode(refresh_token),
        urlencode("openid profile email"),
    );
    post_token(config, body).await.context("刷新令牌失败")
}

async fn post_token(config: &OAuthConfig, body: String) -> Result<TokenSet> {
    let client = reqwest::Client::new();
    let resp = client
        .post(config.token_endpoint())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("请求 token 端点失败")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("token 端点返回 {status}: {text}"));
    }
    serde_json::from_str::<TokenSet>(&text).context("解析 token 响应失败")
}

/// 最小百分号编码（仅保留 RFC3986 unreserved 字符）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_required_params() {
        let cfg = OAuthConfig::default();
        let pending = begin(&cfg);
        let url = &pending.auth_url;
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        // 回调 URL 经过百分号编码
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        // scope 中的空格编码为 %20
        assert!(url.contains("scope=openid%20email%20profile%20offline_access"));
        assert!(url.contains("prompt=login"));
        assert!(!pending.state.is_empty());
    }

    #[test]
    fn urlencode_basics() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
        assert_eq!(urlencode("Aa0-_.~"), "Aa0-_.~");
    }
}
