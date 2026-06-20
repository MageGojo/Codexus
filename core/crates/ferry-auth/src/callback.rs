//! 本地 OAuth 回调服务器：监听 `/auth/callback`，校验 state 并捕获授权码。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    response::Html,
    routing::get,
    Router,
};
use serde::Deserialize;
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

struct CbState {
    expected_state: String,
    tx: Mutex<Option<oneshot::Sender<Result<String, String>>>>,
}

/// 启动回调服务器并阻塞等待授权码（带超时）。成功返回 `code`。
pub async fn wait_for_code(port: u16, expected_state: String, timeout: Duration) -> Result<String> {
    let (tx, rx) = oneshot::channel::<Result<String, String>>();
    let state = Arc::new(CbState {
        expected_state,
        tx: Mutex::new(Some(tx)),
    });

    let app = Router::new()
        .route("/auth/callback", get(handle))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("绑定回调端口 {port} 失败（可能已被占用）"))?;

    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let result = tokio::time::timeout(timeout, rx).await;
    server.abort();

    match result {
        Ok(Ok(Ok(code))) => Ok(code),
        Ok(Ok(Err(e))) => Err(anyhow!("授权失败: {e}")),
        Ok(Err(_)) => Err(anyhow!("回调通道已关闭")),
        Err(_) => Err(anyhow!("等待授权回调超时")),
    }
}

async fn handle(State(state): State<Arc<CbState>>, Query(q): Query<CallbackQuery>) -> Html<String> {
    let outcome: Result<String, String> = if let Some(err) = q.error {
        let desc = q
            .error_description
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        Err(format!("{err}{desc}"))
    } else if q.state.as_deref() != Some(state.expected_state.as_str()) {
        Err("state 校验失败（可能是 CSRF 或并发登录）".to_string())
    } else if let Some(code) = q.code {
        Ok(code)
    } else {
        Err("回调缺少 code 参数".to_string())
    };

    let ok = outcome.is_ok();
    if let Some(tx) = state.tx.lock().await.take() {
        let _ = tx.send(outcome);
    }

    Html(result_page(ok))
}

fn result_page(ok: bool) -> String {
    let (title, msg, color) = if ok {
        (
            "登录成功",
            "你已成功登录Codexus，可以关闭此页面返回应用。",
            "#16a34a",
        )
    } else {
        (
            "登录失败",
            "授权未完成，请回到Codexus 重试。",
            "#dc2626",
        )
    };
    format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Codexus · {title}</title></head>\
<body style=\"font-family:-apple-system,system-ui,sans-serif;display:flex;\
align-items:center;justify-content:center;height:100vh;margin:0;background:#0b0b0c;color:#e5e5e5\">\
<div style=\"text-align:center;max-width:420px;padding:32px\">\
<div style=\"font-size:48px;margin-bottom:16px\">{}</div>\
<h1 style=\"color:{color};font-size:22px;margin:0 0 8px\">{title}</h1>\
<p style=\"color:#a3a3a3;line-height:1.6\">{msg}</p></div></body></html>",
        if ok { "✅" } else { "⚠️" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn captures_code_with_valid_state() {
        let port = free_port();
        let handle = tokio::spawn(wait_for_code(
            port,
            "good-state".to_string(),
            Duration::from_secs(5),
        ));
        tokio::time::sleep(Duration::from_millis(250)).await;

        let url = format!("http://127.0.0.1:{port}/auth/callback?code=the-code&state=good-state");
        let resp = reqwest::get(&url).await.unwrap();
        assert!(resp.status().is_success());

        let code = handle.await.unwrap().unwrap();
        assert_eq!(code, "the-code");
    }

    #[tokio::test]
    async fn rejects_bad_state() {
        let port = free_port();
        let handle = tokio::spawn(wait_for_code(
            port,
            "expected".to_string(),
            Duration::from_secs(5),
        ));
        tokio::time::sleep(Duration::from_millis(250)).await;

        let url = format!("http://127.0.0.1:{port}/auth/callback?code=x&state=wrong");
        let _ = reqwest::get(&url).await.unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
    }
}
