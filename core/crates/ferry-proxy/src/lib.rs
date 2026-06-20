//! ferry-proxy：本地回环代理网关。
//!
//! 接管 Codex 的 `/v1/responses` 流量，按当前供应商协议类型：
//! - `Chat`：用 [`ferry_convert`] 把 Responses 请求转换为 Chat 请求，转发上游，
//!   再把上游响应（流式 SSE / 非流式 JSON）转换回 Responses 形态返回 Codex。
//! - `Responses`：基本透传到上游。

mod pool;
mod tokenizer;

pub use pool::{
    AccountPool, AccountQuota, PoolAccount, PoolAccountStatus, PoolSnapshot, PoolStrategy,
    QuotaWindow, DEFAULT_COOLDOWN_SECS, DEFAULT_FAILURE_THRESHOLD,
};
pub use tokenizer::{count_chat_input_tokens, count_text_tokens};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use ferry_convert::UpstreamApi;
use ferry_store::{NewSessionRecord, SessionStatus, SessionStore};
use futures::StreamExt;
use serde_json::Value;

/// 账号池模式默认上游基址（Codex 官方 ChatGPT 后端）。
pub const CODEX_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// Codex 后端校验的 User-Agent（前缀须为 `codex_cli_rs/`）。
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.47.0";
/// Codex 后端要求的 originator（用于路由到 Codex 通道）。
const CODEX_ORIGINATOR: &str = "codex_cli_rs";
/// 接管 Codex 配置时注入的占位 bearer（与 `ferry-config::DEFAULT_LOCAL_BEARER` 一致）。
/// Codex 会把它当作 Authorization 发给本地代理；代理识别出占位符后改用自身存储的上游 Key，
/// 不会把这个无效值转发给上游。
const PLACEHOLDER_BEARER: &str = "sk-codexferry-local";
/// 旧版（改名前）占位 bearer。用户既有 `~/.codex/config.toml` 可能仍写着它，
/// 这里一并识别为占位符，避免把无效值当真实 Key 转发上游。
const LEGACY_PLACEHOLDER_BEARER: &str = "sk-codeferry-local";

/// 单个供应商配置（MVP：进程内单一活跃供应商；后续由 ferry-config 提供多供应商）。
#[derive(Clone)]
pub struct ProviderConfig {
    /// 上游基址，如 `https://api.deepseek.com/v1`。
    pub base_url: String,
    pub api_key: String,
    pub api_type: UpstreamApi,
    /// 找不到映射时使用的默认模型，如 `deepseek-chat`。
    pub default_model: String,
    /// 客户端模型别名 -> 真实上游模型。
    pub model_map: HashMap<String, String>,
    /// 当前承载请求的账号稳定 id（用于按账号统计 token 用量；可空）。
    #[allow(clippy::doc_markdown)]
    pub account_key: String,
}

impl ProviderConfig {
    /// 解析目标模型：
    /// 1. 命中别名表 -> 改写为映射的真实上游模型；
    /// 2. 未命中但请求非空 -> **透传**（视为用户在 Codex 端用 `/model` 主动选的上游模型，
    ///    如 `deepseek-reasoner`），让"在 Codex 端可选模型"成立；
    /// 3. 请求为空 -> 回退默认模型。
    fn resolve_model(&self, requested: &str) -> String {
        if let Some(mapped) = self.model_map.get(requested) {
            return mapped.clone();
        }
        let requested = requested.trim();
        if requested.is_empty() {
            self.default_model.clone()
        } else {
            requested.to_string()
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

/// 代理路由模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteMode {
    /// 第三方供应商：单 API Key，按需做 Responses <-> Chat 转换（DeepSeek 等）。
    Provider,
    /// Codex 账号池：在多个 ChatGPT 账号间轮询，透传到官方 Responses 上游。
    Pool,
}

impl RouteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Pool => "pool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "provider" => Some(Self::Provider),
            "pool" | "accounts" | "account-pool" => Some(Self::Pool),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    /// 第三方供应商运行时配置（Provider 模式使用）。
    pub provider: Arc<RwLock<ProviderConfig>>,
    /// Codex 账号池（Pool 模式使用）。
    pub pool: Arc<RwLock<AccountPool>>,
    /// 当前路由模式。
    pub mode: Arc<RwLock<RouteMode>>,
    pub http: reqwest::Client,
    pub store: Option<Arc<Mutex<SessionStore>>>,
}

/// 构建 axum 路由。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/responses", post(handle_responses))
        .route("/responses", post(handle_responses))
        .with_state(state)
}

/// 启动代理服务，监听给定地址（阻塞直到服务结束）。
pub async fn serve(addr: SocketAddr, state: AppState) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("ferry-proxy 监听 http://{addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

async fn handle_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mode = state.mode.read().map(|m| *m).unwrap_or(RouteMode::Provider);
    match mode {
        // Provider 模式可用 Codex 传来的 Authorization 作为上游 Key 的兜底来源。
        RouteMode::Provider => handle_via_provider(&state, body, extract_bearer(&headers)).await,
        // Pool 模式用账号池里各账号自己的 access_token，忽略传入头。
        RouteMode::Pool => handle_via_pool(&state, body).await,
    }
}

/// 从请求头解析 `Authorization: Bearer <token>`（去掉占位符与空值）。
fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// 解析转发到上游真正要用的 API Key：
/// 1. 优先用码渡自身存储的供应商 Key（`provider.api_key`，cc-switch 同款：Key 留在工具里、
///    路由注入；Codex 配置里只放占位 bearer）；
/// 2. 存储为空时，兜底用 Codex 传入的真实 bearer（cockpit-tools 同款：把真实 Key 写进
///    Codex 配置的 `experimental_bearer_token`）——但要排除我们自己的占位符；
/// 3. 都没有 -> `None`（调用方返回清晰错误，而不是把空 Bearer 丢给上游拿到看不懂的 401）。
fn resolve_upstream_key(stored: &str, incoming: Option<&str>) -> Option<String> {
    let stored = stored.trim();
    if !stored.is_empty() {
        return Some(stored.to_string());
    }
    let inc = incoming?.trim();
    if inc.is_empty() || inc == PLACEHOLDER_BEARER || inc == LEGACY_PLACEHOLDER_BEARER {
        return None;
    }
    Some(inc.to_string())
}

/// 第三方供应商模式：Responses <-> Chat 转换 / 透传（原有逻辑）。
async fn handle_via_provider(
    state: &AppState,
    body: Value,
    incoming_bearer: Option<String>,
) -> Response {
    let started = Instant::now();
    let mut provider = match state.provider.read() {
        Ok(provider) => provider.clone(),
        Err(e) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("供应商状态不可用: {e}"),
            )
        }
    };
    // 确定上游 Key；缺失则直接给出可读错误，避免上游回 “auth header should be Bearer sk-...”。
    match resolve_upstream_key(&provider.api_key, incoming_bearer.as_deref()) {
        Some(key) => provider.api_key = key,
        None => {
            return error_json(
                StatusCode::BAD_GATEWAY,
                "当前供应商未配置 API Key：请在 Codexus 为该供应商重新「添加账号 / 填写 API Key」后再试（旧版存于系统钥匙串的 Key 在文件存储模式下不可用，需重填一次）",
            )
        }
    }
    let requested = body.get("model").and_then(Value::as_str).unwrap_or("");
    let target_model = provider.resolve_model(requested);
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);

    tracing::info!(
        requested = %requested,
        target = %target_model,
        stream,
        api = ?provider.api_type,
        "收到 /responses 请求"
    );

    match provider.api_type {
        UpstreamApi::Responses => forward_passthrough(state, &provider, &body).await,
        UpstreamApi::Chat => {
            let chat_body = ferry_convert::responses_to_chat_request(&body, &target_model);
            if stream {
                forward_chat_stream(
                    state,
                    &provider,
                    chat_body,
                    target_model,
                    requested.to_string(),
                    body,
                    started,
                )
                .await
            } else {
                forward_chat_once(
                    state,
                    &provider,
                    chat_body,
                    target_model,
                    requested.to_string(),
                    body,
                    started,
                )
                .await
            }
        }
    }
}

/// 非流式：上游 Chat JSON -> Responses JSON。
async fn forward_chat_once(
    state: &AppState,
    provider: &ProviderConfig,
    mut chat_body: Value,
    model: String,
    requested_model: String,
    request_body: Value,
    started: Instant,
) -> Response {
    chat_body["stream"] = Value::Bool(false);
    let est_input = tokenizer::count_chat_input_tokens(&chat_body);
    let url = provider.endpoint("chat/completions");

    let resp = state
        .http
        .post(&url)
        .bearer_auth(&provider.api_key)
        .json(&chat_body)
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            if !status.is_success() {
                let msg = format!("上游返回 {status}: {text}");
                record_session(
                    state,
                    session_record(
                        provider,
                        requested_model,
                        model,
                        false,
                        SessionStatus::Failed,
                        started,
                        request_body,
                        None,
                        None,
                        Some(msg.clone()),
                    ),
                );
                return error_json(StatusCode::BAD_GATEWAY, &msg);
            }
            match serde_json::from_str::<Value>(&text) {
                Ok(chat) => {
                    let responses = ferry_convert::chat_response_to_responses(&chat, &model);
                    let out_text = responses
                        .get("output_text")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let est_output =
                        tokenizer::count_text_tokens(out_text.as_deref().unwrap_or(""));
                    record_session(
                        state,
                        session_record(
                            provider,
                            requested_model,
                            model,
                            false,
                            SessionStatus::Succeeded,
                            started,
                            request_body,
                            Some(responses.clone()),
                            out_text,
                            None,
                        )
                        .with_usage(chat.get("usage"))
                        .with_estimate(est_input, est_output),
                    );
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/json")],
                        responses.to_string(),
                    )
                        .into_response()
                }
                Err(e) => {
                    let msg = format!("解析上游响应失败: {e}");
                    record_session(
                        state,
                        session_record(
                            provider,
                            requested_model,
                            model,
                            false,
                            SessionStatus::Failed,
                            started,
                            request_body,
                            None,
                            None,
                            Some(msg.clone()),
                        ),
                    );
                    error_json(StatusCode::BAD_GATEWAY, &msg)
                }
            }
        }
        Err(e) => {
            let msg = format!("请求上游失败: {e}");
            record_session(
                state,
                session_record(
                    provider,
                    requested_model,
                    model,
                    false,
                    SessionStatus::Failed,
                    started,
                    request_body,
                    None,
                    None,
                    Some(msg.clone()),
                ),
            );
            error_json(StatusCode::BAD_GATEWAY, &msg)
        }
    }
}

/// 流式：上游 Chat SSE -> Responses SSE。
async fn forward_chat_stream(
    state: &AppState,
    provider: &ProviderConfig,
    mut chat_body: Value,
    model: String,
    requested_model: String,
    request_body: Value,
    started: Instant,
) -> Response {
    chat_body["stream"] = Value::Bool(true);
    // 让多数 OpenAI 兼容上游在流末尾返回 usage
    chat_body["stream_options"] = serde_json::json!({ "include_usage": true });
    let est_input = tokenizer::count_chat_input_tokens(&chat_body);
    let url = provider.endpoint("chat/completions");

    let resp = state
        .http
        .post(&url)
        .bearer_auth(&provider.api_key)
        .json(&chat_body)
        .send()
        .await;

    let upstream = match resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            let msg = format!("上游返回 {status}: {text}");
            record_session(
                state,
                session_record(
                    provider,
                    requested_model,
                    model,
                    true,
                    SessionStatus::Failed,
                    started,
                    request_body,
                    None,
                    None,
                    Some(msg.clone()),
                ),
            );
            return error_json(StatusCode::BAD_GATEWAY, &msg);
        }
        Err(e) => {
            let msg = format!("请求上游失败: {e}");
            record_session(
                state,
                session_record(
                    provider,
                    requested_model,
                    model,
                    true,
                    SessionStatus::Failed,
                    started,
                    request_body,
                    None,
                    None,
                    Some(msg.clone()),
                ),
            );
            return error_json(StatusCode::BAD_GATEWAY, &msg);
        }
    };

    let state_for_record = state.clone();
    let provider_for_record = provider.clone();
    let body_stream = async_stream::stream! {
        let mut converter = ferry_convert::StreamConverter::new(model);
        let mut buf: Vec<u8> = Vec::new();
        let mut byte_stream = upstream.bytes_stream();
        let mut usage: Option<Value> = None;
        let mut done = false;
        let mut stream_error: Option<String> = None;

        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("上游流读取错误: {e}");
                    tracing::warn!("{msg}");
                    stream_error = Some(msg);
                    break;
                }
            };
            buf.extend_from_slice(&chunk);

            // 按整行（newline 分隔）解析 SSE
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    done = true;
                    continue;
                }
                if let Ok(json) = serde_json::from_str::<Value>(data) {
                    if let Some(u) = json.get("usage") {
                        if !u.is_null() {
                            usage = Some(u.clone());
                        }
                    }
                    for frame in converter.push_chat_chunk(&json) {
                        yield Ok::<Bytes, std::io::Error>(Bytes::from(frame));
                    }
                }
            }
            if done {
                break;
            }
        }

        for frame in converter.finish(usage.as_ref()) {
            yield Ok(Bytes::from(frame));
        }

        let output_text = converter.accumulated_text().to_string();
        let est_output = tokenizer::count_text_tokens(&output_text);
        let response_json = serde_json::json!({
            "output_text": output_text,
            "finish_reason": converter.finish_reason(),
            "usage": usage.clone(),
        });
        let status = if stream_error.is_some() {
            SessionStatus::Failed
        } else {
            SessionStatus::Succeeded
        };
        record_session(
            &state_for_record,
            session_record(
                &provider_for_record,
                requested_model,
                converter.model().to_string(),
                true,
                status,
                started,
                request_body,
                Some(response_json),
                Some(output_text),
                stream_error,
            )
            .with_usage(usage.as_ref())
            .with_estimate(est_input, est_output),
        );
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| error_json(StatusCode::INTERNAL_SERVER_ERROR, "构建流式响应失败"))
}

/// Responses 协议上游：基本透传（流式与非流式均直接转发字节流）。
async fn forward_passthrough(
    state: &AppState,
    provider: &ProviderConfig,
    body: &Value,
) -> Response {
    let url = provider.endpoint("responses");
    let resp = state
        .http
        .post(&url)
        .bearer_auth(&provider.api_key)
        .json(body)
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::OK);
            let content_type = r
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let byte_stream = r.bytes_stream().map(|res| res.map_err(std::io::Error::other));
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from_stream(byte_stream))
                .unwrap_or_else(|_| {
                    error_json(StatusCode::INTERNAL_SERVER_ERROR, "构建透传响应失败")
                })
        }
        Err(e) => error_json(StatusCode::BAD_GATEWAY, &format!("请求上游失败: {e}")),
    }
}

// ===================== 账号池模式（Codex 多账号轮询 + 故障转移）=====================

/// 账号池模式入口：取一批候选账号（首选 + 故障转移备选），逐个尝试转发到
/// Codex 官方 Responses 上游；鉴权/限流/网络类失败自动切下一个账号。
async fn handle_via_pool(state: &AppState, body: Value) -> Response {
    let started = Instant::now();
    let candidates = match state.pool.write() {
        Ok(mut pool) => pool.attempt_order(),
        Err(e) => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("账号池状态不可用: {e}"),
            )
        }
    };
    if candidates.is_empty() {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "账号池为空：请先添加 Codex 账号，或切回供应商模式",
        );
    }

    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let want_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(true);

    let mut last_error = String::from("无可用账号");
    for account in &candidates {
        if account.access_token.trim().is_empty() {
            last_error = format!("账号 {} 缺少 access_token", account.display_name);
            mark_pool_failure(state, &account.key, &last_error);
            continue;
        }
        match send_codex_request(state, account, &body).await {
            Ok(resp) if resp.status().is_success() => {
                mark_pool_success(state, &account.key);
                capture_pool_quota(state, &account.key, resp.headers());
                tracing::info!(account = %account.display_name, "账号池命中账号转发");
                return if want_stream {
                    forward_codex_stream(state, account, resp, body, requested, started)
                } else {
                    forward_codex_once(state, account, resp, body, requested, started).await
                };
            }
            Ok(resp) => {
                let status = resp.status();
                capture_pool_quota(state, &account.key, resp.headers());
                let text = resp.text().await.unwrap_or_default();
                let msg = format!("上游返回 {status}: {}", truncate(&text, 500));
                tracing::warn!(account = %account.display_name, "账号转发失败，尝试下一个: {msg}");
                mark_pool_failure(state, &account.key, &msg);
                record_session(
                    state,
                    pool_session_record(
                        account,
                        requested.clone(),
                        want_stream,
                        SessionStatus::Failed,
                        started,
                        body.clone(),
                        None,
                        None,
                        Some(msg.clone()),
                    ),
                );
                last_error = msg;
            }
            Err(e) => {
                let msg = format!("请求账号 {} 失败: {e}", account.display_name);
                tracing::warn!("{msg}");
                mark_pool_failure(state, &account.key, &msg);
                last_error = msg;
            }
        }
    }

    error_json(
        StatusCode::BAD_GATEWAY,
        &format!("账号池所有账号转发失败：{last_error}"),
    )
}

fn mark_pool_success(state: &AppState, key: &str) {
    if let Ok(mut pool) = state.pool.write() {
        pool.mark_success(key);
    }
}

/// 把从响应头解析到的配额写回账号池（被动抓头）。
fn capture_pool_quota(state: &AppState, key: &str, headers: &reqwest::header::HeaderMap) {
    if let Some(quota) = parse_quota_headers(headers) {
        if let Ok(mut pool) = state.pool.write() {
            pool.update_quota(key, quota);
        }
    }
}

fn header_f64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
}

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 把可能为毫秒的 epoch 归一化为秒。
fn normalize_epoch_secs(v: f64) -> i64 {
    if v > 1e12 {
        (v / 1000.0) as i64
    } else {
        v as i64
    }
}

/// 从 Codex 响应头解析配额快照（`x-codex-*`）。无任何有效字段时返回 `None`。
pub fn parse_quota_headers(headers: &reqwest::header::HeaderMap) -> Option<AccountQuota> {
    let primary = QuotaWindow {
        used_percent: header_f64(headers, "x-codex-primary-used-percent"),
        window_minutes: header_f64(headers, "x-codex-primary-window-minutes").map(|v| v as i64),
        reset_at: header_f64(headers, "x-codex-primary-reset-at").map(normalize_epoch_secs),
    };
    let secondary = QuotaWindow {
        used_percent: header_f64(headers, "x-codex-secondary-used-percent"),
        window_minutes: header_f64(headers, "x-codex-secondary-window-minutes").map(|v| v as i64),
        reset_at: header_f64(headers, "x-codex-secondary-reset-at").map(normalize_epoch_secs),
    };
    let plan_type = header_str(headers, "x-codex-plan-type");
    let quota = AccountQuota {
        plan_type,
        primary,
        secondary,
        updated_at: Some(chrono::Utc::now().timestamp()),
    };
    if quota.has_data() {
        Some(quota)
    } else {
        None
    }
}

fn mark_pool_failure(state: &AppState, key: &str, err: &str) {
    if let Ok(mut pool) = state.pool.write() {
        pool.mark_failure(key, err);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 向 Codex 官方 Responses 上游发请求：强制流式 + 注入后端必需头。
async fn send_codex_request(
    state: &AppState,
    account: &PoolAccount,
    body: &Value,
) -> reqwest::Result<reqwest::Response> {
    let base = if account.base_url.trim().is_empty() {
        CODEX_BACKEND_BASE_URL
    } else {
        account.base_url.trim_end_matches('/')
    };
    let url = format!("{}/responses", base.trim_end_matches('/'));

    let mut req_body = body.clone();
    if let Some(obj) = req_body.as_object_mut() {
        // Codex 后端期望流式，且拒绝 max_output_tokens 等字段。
        obj.insert("stream".to_string(), Value::Bool(true));
        obj.remove("max_output_tokens");
        let needs_instr = obj
            .get("instructions")
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim().is_empty());
        if needs_instr {
            obj.insert(
                "instructions".to_string(),
                Value::String("You are Codex, OpenAI's coding agent.".to_string()),
            );
        }
    }

    let mut rb = state
        .http
        .post(&url)
        .bearer_auth(&account.access_token)
        .header("originator", CODEX_ORIGINATOR)
        .header(header::USER_AGENT, CODEX_USER_AGENT)
        .header("OpenAI-Beta", "responses=experimental")
        .header(header::ACCEPT, "text/event-stream")
        .json(&req_body);
    if let Some(account_id) = account.account_id.as_deref().filter(|s| !s.is_empty()) {
        rb = rb.header("ChatGPT-Account-ID", account_id);
    }
    rb.send().await
}

/// 流式：原样透传 Codex Responses SSE，旁路解析 output_text / usage 落库。
fn forward_codex_stream(
    state: &AppState,
    account: &PoolAccount,
    upstream: reqwest::Response,
    request_body: Value,
    requested_model: String,
    started: Instant,
) -> Response {
    let state_for_record = state.clone();
    let account = account.clone();
    let body_stream = async_stream::stream! {
        let mut byte_stream = upstream.bytes_stream();
        let mut line_buf: Vec<u8> = Vec::new();
        let mut text = String::new();
        let mut usage: Option<Value> = None;
        let mut stream_error: Option<String> = None;

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    line_buf.extend_from_slice(&bytes);
                    while let Some(pos) = line_buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = line_buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        if let Some(data) = line.trim().strip_prefix("data:") {
                            let data = data.trim();
                            if data.is_empty() || data == "[DONE]" {
                                continue;
                            }
                            if let Ok(ev) = serde_json::from_str::<Value>(data) {
                                accumulate_responses_event(&ev, &mut text, &mut usage);
                            }
                        }
                    }
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
                Err(e) => {
                    let msg = format!("上游流读取错误: {e}");
                    tracing::warn!("{msg}");
                    stream_error = Some(msg);
                    break;
                }
            }
        }

        let status = if stream_error.is_some() {
            SessionStatus::Failed
        } else {
            SessionStatus::Succeeded
        };
        let response_json = serde_json::json!({ "output_text": text, "usage": usage.clone() });
        record_session(
            &state_for_record,
            pool_session_record(
                &account,
                requested_model,
                true,
                status,
                started,
                request_body,
                Some(response_json),
                Some(text),
                stream_error,
            )
            .with_responses_usage(usage.as_ref()),
        );
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "构建账号池流式响应失败")
        })
}

/// 非流式：读取完整上游 SSE，聚合为 Responses JSON 返回（客户端 stream=false）。
async fn forward_codex_once(
    state: &AppState,
    account: &PoolAccount,
    upstream: reqwest::Response,
    request_body: Value,
    requested_model: String,
    started: Instant,
) -> Response {
    let full = match upstream.text().await {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("读取上游响应失败: {e}");
            record_session(
                state,
                pool_session_record(
                    account,
                    requested_model,
                    false,
                    SessionStatus::Failed,
                    started,
                    request_body,
                    None,
                    None,
                    Some(msg.clone()),
                ),
            );
            return error_json(StatusCode::BAD_GATEWAY, &msg);
        }
    };

    let mut text = String::new();
    let mut usage: Option<Value> = None;
    for line in full.lines() {
        if let Some(data) = line.trim().strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<Value>(data) {
                accumulate_responses_event(&ev, &mut text, &mut usage);
            }
        }
    }

    let responses = serde_json::json!({
        "id": format!("resp_pool_{}", started.elapsed().as_nanos()),
        "object": "response",
        "output_text": text,
        "usage": usage.clone(),
    });
    record_session(
        state,
        pool_session_record(
            account,
            requested_model,
            false,
            SessionStatus::Succeeded,
            started,
            request_body,
            Some(responses.clone()),
            Some(text),
            None,
        )
        .with_responses_usage(usage.as_ref()),
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        responses.to_string(),
    )
        .into_response()
}

/// 从一个 Responses SSE 事件累积 output_text 与 usage。
fn accumulate_responses_event(ev: &Value, text: &mut String, usage: &mut Option<Value>) {
    match ev.get("type").and_then(Value::as_str).unwrap_or("") {
        "response.output_text.delta" => {
            if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                text.push_str(d);
            }
        }
        "response.completed" | "response.incomplete" => {
            if let Some(resp) = ev.get("response") {
                if let Some(u) = resp.get("usage").filter(|u| !u.is_null()) {
                    *usage = Some(u.clone());
                }
                if text.is_empty() {
                    if let Some(t) = extract_output_text(resp) {
                        *text = t;
                    }
                }
            }
        }
        _ => {
            if let Some(u) = ev.get("usage").filter(|u| !u.is_null()) {
                *usage = Some(u.clone());
            }
        }
    }
}

/// 从最终 response 对象的 output 数组提取纯文本。
fn extract_output_text(resp: &Value) -> Option<String> {
    let output = resp.get("output")?.as_array()?;
    let mut out = String::new();
    for item in output {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for c in content {
                if let Some(t) = c.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[allow(clippy::too_many_arguments)]
fn pool_session_record(
    account: &PoolAccount,
    requested_model: String,
    stream: bool,
    status: SessionStatus,
    started: Instant,
    request_json: Value,
    response_json: Option<Value>,
    output_text: Option<String>,
    error: Option<String>,
) -> NewSessionRecord {
    // Pool 模式不改写模型（官方上游用 Codex 原生模型名）；provider 字段记账号便于追踪。
    let target_model = requested_model.clone();
    NewSessionRecord {
        provider: format!("codex-pool:{}", account.display_name),
        account: account.key.clone(),
        upstream_api: "responses".to_string(),
        requested_model,
        target_model,
        stream,
        status,
        duration_ms: started.elapsed().as_millis() as i64,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        est_input_tokens: 0,
        est_output_tokens: 0,
        est_total_tokens: 0,
        error,
        request_json,
        response_json,
        output_text,
    }
}

fn error_json(status: StatusCode, msg: &str) -> Response {
    tracing::warn!("代理错误: {msg}");
    let body = serde_json::json!({
        "error": { "message": msg, "type": "ferry_proxy_error" }
    });
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

#[allow(clippy::too_many_arguments)]
fn session_record(
    provider: &ProviderConfig,
    requested_model: String,
    target_model: String,
    stream: bool,
    status: SessionStatus,
    started: Instant,
    request_json: Value,
    response_json: Option<Value>,
    output_text: Option<String>,
    error: Option<String>,
) -> NewSessionRecord {
    NewSessionRecord {
        provider: provider.base_url.clone(),
        account: provider.account_key.clone(),
        upstream_api: format!("{:?}", provider.api_type).to_lowercase(),
        requested_model,
        target_model,
        stream,
        status,
        duration_ms: started.elapsed().as_millis() as i64,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        est_input_tokens: 0,
        est_output_tokens: 0,
        est_total_tokens: 0,
        error,
        request_json,
        response_json,
        output_text,
    }
}

trait SessionRecordExt {
    /// Chat 协议 usage（prompt_tokens / completion_tokens）。
    fn with_usage(self, usage: Option<&Value>) -> Self;
    /// Responses 协议 usage（input_tokens / output_tokens）。
    fn with_responses_usage(self, usage: Option<&Value>) -> Self;
    /// 本地分词器估算的 token（用于与上游上报对比、识别掺假）。
    fn with_estimate(self, est_input: i64, est_output: i64) -> Self;
}

impl SessionRecordExt for NewSessionRecord {
    fn with_usage(mut self, usage: Option<&Value>) -> Self {
        if let Some(u) = usage {
            self.input_tokens = u.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0);
            self.output_tokens = u
                .get("completion_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            self.total_tokens = u.get("total_tokens").and_then(Value::as_i64).unwrap_or(0);
        }
        self
    }

    fn with_responses_usage(mut self, usage: Option<&Value>) -> Self {
        if let Some(u) = usage {
            let input = u.get("input_tokens").and_then(Value::as_i64).unwrap_or(0);
            let output = u.get("output_tokens").and_then(Value::as_i64).unwrap_or(0);
            let total = u
                .get("total_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(input + output);
            self.input_tokens = input;
            self.output_tokens = output;
            self.total_tokens = total;
        }
        self
    }

    fn with_estimate(mut self, est_input: i64, est_output: i64) -> Self {
        self.est_input_tokens = est_input;
        self.est_output_tokens = est_output;
        self.est_total_tokens = est_input + est_output;
        self
    }
}

fn record_session(state: &AppState, record: NewSessionRecord) {
    let Some(store) = &state.store else {
        return;
    };
    match store.lock() {
        Ok(store) => {
            if let Err(e) = store.insert_session(&record) {
                tracing::warn!("写入会话记录失败: {e}");
            }
        }
        Err(e) => tracing::warn!("会话存储锁已损坏: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(default_model: &str, aliases: &[(&str, &str)]) -> ProviderConfig {
        ProviderConfig {
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            api_type: UpstreamApi::Chat,
            default_model: default_model.to_string(),
            model_map: aliases
                .iter()
                .map(|(f, t)| (f.to_string(), t.to_string()))
                .collect(),
            account_key: String::new(),
        }
    }

    #[test]
    fn resolve_model_maps_known_alias() {
        let p = provider("deepseek-chat", &[("gpt-5-codex", "deepseek-chat")]);
        assert_eq!(p.resolve_model("gpt-5-codex"), "deepseek-chat");
    }

    #[test]
    fn resolve_model_passes_through_unknown_for_codex_choice() {
        // 未在别名表的模型 -> 透传（支持在 Codex 端用 /model 主动选上游模型）。
        let p = provider("deepseek-chat", &[("gpt-5-codex", "deepseek-chat")]);
        assert_eq!(p.resolve_model("deepseek-reasoner"), "deepseek-reasoner");
    }

    #[test]
    fn resolve_model_empty_falls_back_to_default() {
        let p = provider("deepseek-chat", &[]);
        assert_eq!(p.resolve_model(""), "deepseek-chat");
        assert_eq!(p.resolve_model("   "), "deepseek-chat");
    }

    #[test]
    fn resolve_upstream_key_prefers_stored() {
        assert_eq!(
            resolve_upstream_key("sk-stored", Some("sk-incoming")).as_deref(),
            Some("sk-stored")
        );
    }

    #[test]
    fn resolve_upstream_key_falls_back_to_incoming_real_key() {
        // 存储为空 -> 用 Codex 传来的真实 bearer（cockpit-tools 写法）。
        assert_eq!(
            resolve_upstream_key("", Some("sk-real-from-config")).as_deref(),
            Some("sk-real-from-config")
        );
    }

    #[test]
    fn resolve_upstream_key_rejects_placeholder_and_empty() {
        assert!(resolve_upstream_key("", Some(PLACEHOLDER_BEARER)).is_none());
        assert!(resolve_upstream_key("", Some(LEGACY_PLACEHOLDER_BEARER)).is_none());
        assert!(resolve_upstream_key("   ", Some("   ")).is_none());
        assert!(resolve_upstream_key("", None).is_none());
    }

    #[test]
    fn extract_bearer_parses_and_filters() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer sk-abc".parse().unwrap());
        assert_eq!(extract_bearer(&h).as_deref(), Some("sk-abc"));

        let mut lower = HeaderMap::new();
        lower.insert(header::AUTHORIZATION, "bearer sk-low".parse().unwrap());
        assert_eq!(extract_bearer(&lower).as_deref(), Some("sk-low"));

        assert!(extract_bearer(&HeaderMap::new()).is_none());
    }

    #[test]
    fn route_mode_parse_and_str() {
        assert_eq!(RouteMode::parse("provider"), Some(RouteMode::Provider));
        assert_eq!(RouteMode::parse("pool"), Some(RouteMode::Pool));
        assert_eq!(RouteMode::parse("accounts"), Some(RouteMode::Pool));
        assert_eq!(RouteMode::parse("POOL"), Some(RouteMode::Pool));
        assert_eq!(RouteMode::parse("xxx"), None);
        assert_eq!(RouteMode::Provider.as_str(), "provider");
        assert_eq!(RouteMode::Pool.as_str(), "pool");
    }

    #[test]
    fn accumulate_responses_event_extracts_delta_and_usage() {
        let mut text = String::new();
        let mut usage = None;
        accumulate_responses_event(
            &serde_json::json!({"type":"response.output_text.delta","delta":"Hello"}),
            &mut text,
            &mut usage,
        );
        accumulate_responses_event(
            &serde_json::json!({"type":"response.output_text.delta","delta":" world"}),
            &mut text,
            &mut usage,
        );
        accumulate_responses_event(
            &serde_json::json!({
                "type":"response.completed",
                "response": { "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15} }
            }),
            &mut text,
            &mut usage,
        );
        assert_eq!(text, "Hello world");
        let u = usage.unwrap();
        assert_eq!(u["input_tokens"], 10);
        assert_eq!(u["total_tokens"], 15);
    }

    #[test]
    fn accumulate_extracts_text_from_final_output_when_no_delta() {
        let mut text = String::new();
        let mut usage = None;
        accumulate_responses_event(
            &serde_json::json!({
                "type":"response.completed",
                "response": {
                    "usage": {"input_tokens": 1, "output_tokens": 2},
                    "output": [ { "content": [ { "type":"output_text", "text":"final text" } ] } ]
                }
            }),
            &mut text,
            &mut usage,
        );
        assert_eq!(text, "final text");
    }

    #[test]
    fn with_responses_usage_computes_total_when_missing() {
        let rec = NewSessionRecord {
            provider: "p".into(),
            account: String::new(),
            upstream_api: "responses".into(),
            requested_model: "m".into(),
            target_model: "m".into(),
            stream: true,
            status: SessionStatus::Succeeded,
            duration_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            est_input_tokens: 0,
            est_output_tokens: 0,
            est_total_tokens: 0,
            error: None,
            request_json: serde_json::json!({}),
            response_json: None,
            output_text: None,
        };
        let usage = serde_json::json!({"input_tokens": 7, "output_tokens": 3});
        let rec = rec.with_responses_usage(Some(&usage));
        assert_eq!(rec.input_tokens, 7);
        assert_eq!(rec.output_tokens, 3);
        assert_eq!(rec.total_tokens, 10);
    }
}
