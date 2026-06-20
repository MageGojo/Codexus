//! 解析 ChatGPT id_token（JWT）以提取 email 与 account_id。
//!
//! 仅解码 payload 读取 claims，不校验签名（凭据经 TLS 从 OpenAI 取得，签名由其负责）。

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct IdTokenClaims {
    pub email: Option<String>,
    pub account_id: Option<String>,
    /// ChatGPT 订阅方案类型（如 `plus` / `pro` / `team` / `enterprise` /
    /// `self_serve_business_usage_based` 等），用于在账号卡片显示 plan 徽章。
    pub plan_type: Option<String>,
    pub raw: Value,
}

/// 解析 id_token，提取常用 claims。
pub fn parse_id_token(id_token: &str) -> Result<IdTokenClaims> {
    let payload = id_token
        .split('.')
        .nth(1)
        .context("id_token 不是合法的 JWT（缺少 payload 段）")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .context("解码 JWT payload 失败")?;
    let claims: Value = serde_json::from_slice(&bytes).context("解析 JWT claims 失败")?;

    Ok(IdTokenClaims {
        email: find_email(&claims),
        account_id: find_account_id(&claims),
        plan_type: find_plan_type(&claims),
        raw: claims,
    })
}

fn find_email(c: &Value) -> Option<String> {
    if let Some(e) = c.get("email").and_then(Value::as_str) {
        return Some(e.to_string());
    }
    c.get("https://api.openai.com/profile")
        .and_then(|p| p.get("email"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn find_account_id(c: &Value) -> Option<String> {
    if let Some(a) = c
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
    {
        return Some(a.to_string());
    }
    c.get("chatgpt_account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 提取订阅方案（plan）：优先 OpenAI 命名空间下的 `chatgpt_plan_type`，回退顶层。
fn find_plan_type(c: &Value) -> Option<String> {
    if let Some(p) = c
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
    {
        return Some(p.to_string());
    }
    c.get("chatgpt_plan_type")
        .or_else(|| c.get("plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_jwt(payload: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        format!("{header}.{body}.sig")
    }

    #[test]
    fn parses_top_level_email() {
        let jwt = make_jwt(&json!({"email":"a@b.com","chatgpt_account_id":"acc-1"}));
        let c = parse_id_token(&jwt).unwrap();
        assert_eq!(c.email.as_deref(), Some("a@b.com"));
        assert_eq!(c.account_id.as_deref(), Some("acc-1"));
    }

    #[test]
    fn parses_namespaced_claims() {
        let jwt = make_jwt(&json!({
            "https://api.openai.com/profile": {"email":"x@y.com"},
            "https://api.openai.com/auth": {"chatgpt_account_id":"acc-2"}
        }));
        let c = parse_id_token(&jwt).unwrap();
        assert_eq!(c.email.as_deref(), Some("x@y.com"));
        assert_eq!(c.account_id.as_deref(), Some("acc-2"));
    }

    #[test]
    fn parses_plan_type_from_auth_namespace() {
        let jwt = make_jwt(&json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-3",
                "chatgpt_plan_type": "plus"
            }
        }));
        let c = parse_id_token(&jwt).unwrap();
        assert_eq!(c.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn parses_plan_type_top_level_fallback() {
        let jwt = make_jwt(&json!({"chatgpt_plan_type":"team"}));
        let c = parse_id_token(&jwt).unwrap();
        assert_eq!(c.plan_type.as_deref(), Some("team"));
    }
}
