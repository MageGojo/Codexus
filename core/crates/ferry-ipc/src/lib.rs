//! ferry-ipc：本地管理 API。
//!
//! 面向后续 Flutter GUI，提供供应商、账号、会话和 Codex 配置接管等管理能力。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use axum::{
    extract::{Path as AxumPath, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use ferry_auth::{
    import_codex_account, import_codex_from_json, login_with_api_key, login_with_api_key_for,
    login_with_browser, login_with_tokens, parse_codex_cli_auth, Account, AccountMeta, AuthMode,
    AuthStore, OAuthConfig,
};
use ferry_config::{
    all_providers, entry_models, find_provider_preset, provider_presets, resolve_provider,
    AppSettings, CodexConfig, CodexPreferences, CustomProvider, ProviderApi, ProviderEntry,
    ProviderStore, SettingsStore, TakeoverParams, DEFAULT_PROVIDER_KEY,
};
use chrono::{Duration, Utc};
use ferry_convert::UpstreamApi;
use ferry_proxy::{
    AccountPool, AccountQuota, PoolAccount, PoolSnapshot, PoolStrategy, ProviderConfig, QuotaWindow,
    RouteMode, CODEX_BACKEND_BASE_URL,
};

/// apizero 服务密钥在凭据存储中的名字。
const APIZERO_SERVICE: &str = "apizero";
/// apizero 接口基址。
const APIZERO_BASE: &str = "https://v1.apizero.cn/api";
use ferry_store::{
    AccountUsage, DayAccounts, DayTokens, ProviderUsage, SessionStore,
};
use serde::{Deserialize, Serialize};

/// IPC 服务状态。
#[derive(Clone, Default)]
pub struct IpcState {
    pub auth_store: Option<AuthStore>,
    pub session_store: Option<Arc<Mutex<SessionStore>>>,
    pub codex_config: Option<CodexConfig>,
    pub active_provider: Option<Arc<RwLock<ProviderConfig>>>,
    pub provider_store: Option<ProviderStore>,
    /// Codex 账号池（Pool 模式运行时状态）。
    pub account_pool: Option<Arc<RwLock<AccountPool>>>,
    /// 当前路由模式（provider / pool）。
    pub route_mode: Option<Arc<RwLock<RouteMode>>>,
    /// 应用设置存储（天气城市、开关等非密钥设置）。
    pub settings_store: Option<SettingsStore>,
    /// 管理 API 本地鉴权 token；`Some` 时强制 Bearer 校验（`/ipc/health` 除外）。
    pub auth_token: Option<String>,
    /// 出站 HTTP 客户端（天气/诗词/配额探测等集成调用）。
    pub http_client: reqwest::Client,
    /// 天气结果缓存（key = `type|city请求参数`，TTL 1 小时；避免每次进仪表盘/切页都打外部天气与 IP 定位接口）。
    pub weather_cache: Arc<RwLock<HashMap<String, (std::time::Instant, WeatherResponse)>>>,
}

/// 构建管理 API 路由。
pub fn router(state: IpcState) -> Router {
    let token = state.auth_token.clone();
    Router::new()
        .route("/ipc/health", get(health))
        .route("/ipc/settings", get(get_settings).post(save_settings))
        .route("/ipc/settings/apizero-key", post(set_apizero_key))
        .route("/ipc/integrations/weather", get(get_weather))
        .route("/ipc/integrations/poem", post(get_poem))
        .route("/ipc/runtime/pool/refresh-quota", post(refresh_pool_quota))
        .route("/ipc/runtime/pool/strategy", post(set_pool_strategy))
        .route("/ipc/providers", get(list_providers).post(upsert_provider))
        .route("/ipc/providers/export", get(export_providers))
        .route("/ipc/providers/import", post(import_providers))
        .route("/ipc/providers/{id}/api-key", post(set_provider_api_key))
        .route("/ipc/providers/{id}/models", post(fetch_provider_models))
        .route("/ipc/providers/{id}", delete(delete_provider))
        .route("/ipc/accounts", get(list_accounts))
        .route("/ipc/accounts/api-key", post(add_api_key_account))
        .route("/ipc/accounts/{id}/update", post(update_account))
        .route("/ipc/accounts/{id}/use", post(use_account))
        .route("/ipc/accounts/codex-login", post(codex_login))
        .route("/ipc/accounts/codex-token", post(add_codex_token_account))
        .route(
            "/ipc/accounts/import-codex-local",
            post(import_codex_local_account),
        )
        .route("/ipc/accounts/import-json", post(import_codex_json_accounts))
        .route("/ipc/accounts/{id}", delete(delete_account))
        .route(
            "/ipc/runtime/provider",
            get(active_provider).post(switch_provider),
        )
        .route("/ipc/runtime/pool", get(get_pool))
        .route("/ipc/sessions", get(list_sessions))
        .route("/ipc/sessions/{id}", get(get_session))
        .route("/ipc/stats", get(stats))
        .route("/ipc/codex/status", get(codex_status))
        .route("/ipc/codex/takeover", post(codex_takeover))
        .route("/ipc/codex/release", post(codex_release))
        .route("/ipc/codex/restore-latest", post(codex_restore_latest))
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let token = token.clone();
            async move { auth_guard(token, req, next).await }
        }))
        .with_state(state)
}

/// 启动管理 API 服务。
pub async fn serve(addr: SocketAddr, state: IpcState) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("ferry-ipc 监听 http://{addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// 本地鉴权中间件：除 `/ipc/health` 外，`auth_token` 存在时要求 `Authorization: Bearer`，
/// 并校验 Host 为回环（缓解浏览器 DNS rebinding 对本地管理面的越权访问）。
async fn auth_guard(token: Option<String>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/ipc/health" {
        return next.run(req).await;
    }
    let Some(expected) = token else {
        return next.run(req).await;
    };
    if let Some(host) = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        if !host_is_loopback(host) {
            return unauthorized("非法 Host：管理 API 仅限本机回环访问");
        }
    }
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim);
    if provided == Some(expected.as_str()) {
        next.run(req).await
    } else {
        unauthorized("管理 API 需要有效的本地 token")
    }
}

fn host_is_loopback(host: &str) -> bool {
    let host = host.trim();
    let h = if let Some(rest) = host.strip_prefix('[') {
        // 形如 [::1]:15722 或 [::1]：取方括号内的 IPv6。
        rest.split(']').next().unwrap_or("")
    } else if host.matches(':').count() == 1 {
        // 形如 127.0.0.1:15722 / localhost:15722：去掉端口。
        host.split(':').next().unwrap_or(host)
    } else {
        // 裸 IPv6（如 ::1）或纯主机名。
        host
    };
    matches!(h, "127.0.0.1" | "localhost" | "::1")
}

fn unauthorized(message: &str) -> Response {
    let body = serde_json::json!({
        "error": { "message": message, "type": "ferry_ipc_unauthorized" }
    });
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

// ===================== 应用设置 =====================

fn apizero_key(state: &IpcState) -> Option<String> {
    state
        .auth_store
        .as_ref()
        .and_then(|s| s.get_service_key(APIZERO_SERVICE).ok().flatten())
        .filter(|k| !k.trim().is_empty())
}

fn settings_response(state: &IpcState, settings: AppSettings) -> SettingsResponse {
    SettingsResponse {
        apizero_key_configured: apizero_key(state).is_some(),
        settings,
    }
}

async fn get_settings(State(state): State<IpcState>) -> Json<SettingsResponse> {
    let settings = state
        .settings_store
        .as_ref()
        .map(|s| s.load())
        .unwrap_or_default();
    Json(settings_response(&state, settings))
}

async fn save_settings(
    State(state): State<IpcState>,
    Json(req): Json<AppSettings>,
) -> std::result::Result<Json<SettingsResponse>, ApiError> {
    let store = state
        .settings_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("设置存储不可用"))?;
    store.save(&req).map_err(ApiError::internal)?;
    // 设置里的「配额感知」开关同步到运行时账号池。
    if let Some(cell) = state.account_pool.as_ref() {
        if let Ok(mut g) = cell.write() {
            g.set_strategy(if req.pool_quota_aware {
                PoolStrategy::QuotaAware
            } else {
                PoolStrategy::RoundRobin
            });
        }
    }
    Ok(Json(settings_response(&state, req)))
}

async fn set_apizero_key(
    State(state): State<IpcState>,
    Json(req): Json<ApizeroKeyRequest>,
) -> std::result::Result<Json<OkResponse>, ApiError> {
    let auth = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("凭据存储不可用"))?;
    let key = req.api_key.trim();
    if key.is_empty() {
        auth.delete_service_key(APIZERO_SERVICE)
            .map_err(ApiError::internal)?;
    } else {
        auth.set_service_key(APIZERO_SERVICE, key)
            .map_err(ApiError::internal)?;
    }
    Ok(Json(OkResponse { ok: true }))
}

// ===================== 生活化集成：天气 / 古诗词 =====================

async fn get_weather(
    State(state): State<IpcState>,
    Query(q): Query<WeatherQuery>,
) -> std::result::Result<Json<WeatherResponse>, ApiError> {
    let kind = q
        .r#type
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("realtime")
        .to_string();
    // 缓存键用「原始请求参数」（type|显式 city；自动定位时 city 为空）。命中 1 小时内
    // 直接返回——连 IP 定位与上游天气请求都省掉，实现「每小时才查询一次」。
    let req_city = q
        .city
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("")
        .to_string();
    let cache_key = format!("{kind}|{req_city}");
    if !q.refresh.unwrap_or(false) {
        if let Some(cached) = weather_cache_get(&state, &cache_key) {
            return Ok(Json(cached));
        }
    }

    let settings = state
        .settings_store
        .as_ref()
        .map(|s| s.load())
        .unwrap_or_default();
    // 城市优先级：显式查询 city > （自动定位开启 / 未设手动城市 → IP 定位）> 手动城市 > 北京兜底。
    let city = match q.city.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => c.to_string(),
        None => {
            if settings.weather_auto_locate || settings.weather_city.trim().is_empty() {
                detect_city(&state)
                    .await
                    .unwrap_or_else(|| "北京".to_string())
            } else {
                settings.weather_city.clone()
            }
        }
    };

    let mut rb = state
        .http_client
        .get(format!("{APIZERO_BASE}/weather"))
        .query(&[("type", kind.as_str()), ("city", city.as_str())]);
    if let Some(k) = apizero_key(&state) {
        rb = rb.header("X-API-Key", k);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("天气请求失败: {e}")))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("天气响应解析失败: {e}")))?;
    let code = json.get("code").and_then(serde_json::Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = json
            .get("msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("未知错误");
        return Err(ApiError::bad_gateway(format!("天气接口错误[{code}]: {msg}")));
    }
    let data = json.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let result = normalize_weather(&data, &city);
    weather_cache_put(&state, &cache_key, &result);
    Ok(Json(result))
}

/// 天气缓存 TTL：1 小时（每小时才真正查询一次外部接口）。
const WEATHER_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// 读天气缓存：命中且未超过 TTL 返回克隆，否则 None。
fn weather_cache_get(state: &IpcState, key: &str) -> Option<WeatherResponse> {
    let cache = state.weather_cache.read().ok()?;
    let (at, resp) = cache.get(key)?;
    if at.elapsed() < WEATHER_TTL {
        Some(resp.clone())
    } else {
        None
    }
}

/// 写天气缓存（记录当前时刻）。
fn weather_cache_put(state: &IpcState, key: &str, resp: &WeatherResponse) {
    if let Ok(mut cache) = state.weather_cache.write() {
        cache.insert(key.to_string(), (std::time::Instant::now(), resp.clone()));
    }
}

/// 按本机出口 IP 自动定位当前城市（ip-api.com 免费接口，`lang=zh-CN` 返回中文城市名）。
/// 失败返回 `None`，由调用方兜底。
async fn detect_city(state: &IpcState) -> Option<String> {
    let resp = state
        .http_client
        .get("http://ip-api.com/json/?fields=status,city,regionName&lang=zh-CN")
        .timeout(std::time::Duration::from_secs(6))
        .send()
        .await
        .ok()?;
    let v: serde_json::Value = resp.json().await.ok()?;
    if v.get("status").and_then(serde_json::Value::as_str) != Some("success") {
        return None;
    }
    let pick = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    pick("city").or_else(|| pick("regionName"))
}

async fn get_poem(
    State(state): State<IpcState>,
    Json(req): Json<PoemRequest>,
) -> std::result::Result<Json<PoemResponse>, ApiError> {
    let mut body = serde_json::Map::new();
    if let Some(t) = req.r#type.filter(|t| !t.trim().is_empty()) {
        body.insert("type".to_string(), serde_json::Value::String(t));
    }
    let mut rb = state
        .http_client
        .post(format!("{APIZERO_BASE}/shici"))
        .json(&serde_json::Value::Object(body));
    if let Some(k) = apizero_key(&state) {
        rb = rb.header("X-Api-Key", k);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("古诗词请求失败: {e}")))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::bad_gateway(format!("古诗词响应解析失败: {e}")))?;
    let code = json.get("code").and_then(serde_json::Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = json
            .get("msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("未知错误");
        return Err(ApiError::bad_gateway(format!("古诗词接口错误[{code}]: {msg}")));
    }
    let data = json.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let s = |k: &str| data.get(k).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    Ok(Json(PoemResponse {
        content: s("content"),
        origin: s("origin"),
        author: s("author"),
        category: s("category"),
    }))
}

/// 把 apizero 天气 `data` 归一化为前端易用的摘要（不下发原始大对象）。
fn normalize_weather(data: &serde_json::Value, fallback_city: &str) -> WeatherResponse {
    let summary = data.get("summary");
    let g = |obj: Option<&serde_json::Value>, k: &str| {
        obj.and_then(|o| o.get(k))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let gf = |obj: Option<&serde_json::Value>, k: &str| {
        obj.and_then(|o| o.get(k)).and_then(serde_json::Value::as_f64)
    };
    let wind = summary.and_then(|s| s.get("wind"));
    let air = summary.and_then(|s| s.get("air_quality"));
    let city = data
        .get("location")
        .and_then(|l| l.get("city"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| fallback_city.to_string());

    let alerts = data
        .get("alerts")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|a| WeatherAlert {
                    title: a.get("title").and_then(serde_json::Value::as_str).unwrap_or("").to_string(),
                    color: a.get("color").and_then(serde_json::Value::as_str).unwrap_or("").to_string(),
                    level: a.get("level").and_then(serde_json::Value::as_str).unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    WeatherResponse {
        city,
        skycon: g(summary, "skycon"),
        emoji: g(summary, "skycon_emoji"),
        skycon_code: g(summary, "skycon_code"),
        temperature: gf(summary, "temperature"),
        apparent_temperature: gf(summary, "apparent_temperature"),
        humidity_percent: gf(summary, "humidity_percent"),
        visibility_km: gf(summary, "visibility_km"),
        wind_text: g(wind, "direction_text"),
        wind_level_text: g(wind, "level_text"),
        aqi: gf(air, "aqi"),
        aqi_level: g(air, "level"),
        aqi_color: g(air, "level_color"),
        pm25: gf(air, "pm25"),
        forecast_keypoint: data
            .get("forecast_keypoint")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        alerts,
    }
}

// ===================== 账号池：配额刷新 / 调度策略 =====================

async fn set_pool_strategy(
    State(state): State<IpcState>,
    Json(req): Json<StrategyRequest>,
) -> std::result::Result<Json<PoolResponse>, ApiError> {
    let strategy = PoolStrategy::parse(&req.strategy)
        .ok_or_else(|| ApiError::bad_request("未知策略，应为 round_robin 或 quota_aware"))?;
    let cell = pool_cell(&state)?;
    {
        let mut guard = cell
            .write()
            .map_err(|e| ApiError::internal(anyhow::anyhow!("账号池锁已损坏: {e}")))?;
        guard.set_strategy(strategy);
    }
    if let Some(store) = state.settings_store.as_ref() {
        let mut s = store.load();
        s.pool_quota_aware = strategy == PoolStrategy::QuotaAware;
        let _ = store.save(&s);
    }
    Ok(Json(pool_snapshot(&state)?))
}

/// 主动逐账号探测 Codex 配额（`/usage`）并刷新账号池；顺带做健康检查（401 计失败）。
async fn refresh_pool_quota(
    State(state): State<IpcState>,
) -> std::result::Result<Json<PoolResponse>, ApiError> {
    let cell = pool_cell(&state)?;
    // 探测前先从账号存储重装账号池，确保纳入最新（含刚添加 / OAuth 直连）的 Codex 账号，
    // 否则新账号不在池里就探测不到其额度（用户报「额度没更新」的一个原因）。
    if let Some(auth) = state.auth_store.as_ref() {
        let accounts = load_pool_accounts(auth);
        if let Ok(mut guard) = cell.write() {
            guard.set_accounts(accounts);
        }
    }
    let targets = {
        let guard = cell
            .read()
            .map_err(|e| ApiError::internal(anyhow::anyhow!("账号池锁已损坏: {e}")))?;
        guard.accounts_snapshot()
    };
    for acc in targets {
        match probe_codex_usage(&state.http_client, &acc).await {
            Ok(Some(quota)) => {
                if let Ok(mut guard) = cell.write() {
                    guard.update_quota(&acc.key, quota);
                    guard.mark_success(&acc.key);
                }
            }
            Ok(None) => {}
            Err(ProbeError::Unauthorized) => {
                if let Ok(mut guard) = cell.write() {
                    guard.mark_failure(&acc.key, "配额探测返回 401（token 可能失效）");
                }
            }
            Err(ProbeError::Other(e)) => {
                tracing::warn!(account = %acc.display_name, "配额探测失败: {e}");
            }
        }
    }
    Ok(Json(pool_snapshot(&state)?))
}

enum ProbeError {
    Unauthorized,
    Other(String),
}

/// ChatGPT 网页后端 5h/7d 额度端点（对照 cockpit-tools，与 codex backend 不同路径）。
const WHAM_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
/// 网页态请求用的浏览器 UA（对照 cockpit-tools，wham/usage 需网页头才稳定返回）。
const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

/// 探测单账号 Codex 5h/7d 额度：调 `wham/usage`（cockpit 同款，带网页头 + ChatGPT-Account-Id），
/// 优先响应头、回退响应体（best-effort）。
async fn probe_codex_usage(
    http: &reqwest::Client,
    acc: &PoolAccount,
) -> std::result::Result<Option<AccountQuota>, ProbeError> {
    let mut rb = http
        .get(WHAM_USAGE_URL)
        .bearer_auth(&acc.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::REFERER, "https://chatgpt.com/")
        .header(reqwest::header::USER_AGENT, CHATGPT_WEB_USER_AGENT);
    if let Some(id) = acc.account_id.as_deref().filter(|s| !s.is_empty()) {
        rb = rb.header("ChatGPT-Account-Id", id);
    }
    let resp = rb.send().await.map_err(|e| ProbeError::Other(e.to_string()))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ProbeError::Unauthorized);
    }
    let header_quota = ferry_proxy::parse_quota_headers(resp.headers());
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    let body_quota = parse_usage_body(&body);
    Ok(merge_quota(header_quota, body_quota))
}

/// 合并表头与表体配额（表头优先，表体补缺）。
fn merge_quota(header: Option<AccountQuota>, body: Option<AccountQuota>) -> Option<AccountQuota> {
    match (header, body) {
        (Some(mut h), Some(b)) => {
            h.plan_type = h.plan_type.or(b.plan_type);
            if h.primary.used_percent.is_none() {
                h.primary = b.primary;
            }
            if h.secondary.used_percent.is_none() {
                h.secondary = b.secondary;
            }
            Some(h)
        }
        (Some(h), None) => Some(h),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// 容错解析 `/usage` 响应体里的配额（不同后端字段名略有差异）。
fn parse_usage_body(json: &serde_json::Value) -> Option<AccountQuota> {
    let root = json
        .get("data")
        .or_else(|| json.get("rate_limits"))
        .or_else(|| json.get("usage"))
        .unwrap_or(json);
    let plan_type = root
        .get("plan_type")
        .or_else(|| root.get("plan"))
        .or_else(|| json.get("plan_type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    // cockpit-tools 同款：5h/7d 在 `rate_limit.primary_window` / `secondary_window`；
    // 兼容旧的顶层 `primary` / `primary_window` 字段。
    let rate_limit = json.get("rate_limit").or_else(|| root.get("rate_limit"));
    let primary = parse_usage_window(
        rate_limit
            .and_then(|r| r.get("primary_window"))
            .or_else(|| root.get("primary"))
            .or_else(|| root.get("primary_window")),
    );
    let secondary = parse_usage_window(
        rate_limit
            .and_then(|r| r.get("secondary_window"))
            .or_else(|| root.get("secondary"))
            .or_else(|| root.get("secondary_window")),
    );
    let quota = AccountQuota {
        plan_type,
        primary: primary.unwrap_or_default(),
        secondary: secondary.unwrap_or_default(),
        updated_at: Some(Utc::now().timestamp()),
    };
    if quota.has_data() {
        Some(quota)
    } else {
        None
    }
}

fn parse_usage_window(win: Option<&serde_json::Value>) -> Option<QuotaWindow> {
    let w = win?;
    let used_percent = w
        .get("used_percent")
        .or_else(|| w.get("usage_percent"))
        .and_then(serde_json::Value::as_f64);
    let window_minutes = w
        .get("window_minutes")
        .and_then(serde_json::Value::as_i64)
        .or_else(|| {
            w.get("limit_window_seconds")
                .or_else(|| w.get("window_seconds"))
                .and_then(serde_json::Value::as_i64)
                .map(|s| s / 60)
        });
    let reset_at = w
        .get("reset_at")
        .or_else(|| w.get("resets_at"))
        .and_then(serde_json::Value::as_i64)
        .map(|v| if v > 1_000_000_000_000 { v / 1000 } else { v })
        .or_else(|| {
            w.get("resets_in_seconds")
                .or_else(|| w.get("reset_after_seconds"))
                .and_then(serde_json::Value::as_i64)
                .map(|s| Utc::now().timestamp() + s)
        });
    let win = QuotaWindow {
        used_percent,
        window_minutes,
        reset_at,
    };
    if win.used_percent.is_none() && win.window_minutes.is_none() && win.reset_at.is_none() {
        None
    } else {
        Some(win)
    }
}

async fn list_providers(State(state): State<IpcState>) -> Json<Vec<ProviderEntry>> {
    let entries = match state.provider_store.as_ref() {
        Some(store) => all_providers(store).unwrap_or_else(|e| {
            tracing::warn!("读取自定义供应商失败，仅返回内置预设: {e}");
            builtin_entries()
        }),
        None => builtin_entries(),
    };
    Json(entries)
}

fn builtin_entries() -> Vec<ProviderEntry> {
    provider_presets()
        .iter()
        .map(ProviderEntry::from_preset)
        .collect()
}

async fn upsert_provider(
    State(state): State<IpcState>,
    Json(req): Json<CustomProvider>,
) -> std::result::Result<Json<ProviderEntry>, ApiError> {
    let store = state
        .provider_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("供应商存储不可用"))?;
    let saved = store
        .upsert(req)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut entry = ProviderEntry::from_custom(saved);
    // 覆盖内置预设时仍标记为内置（供前端给「恢复默认」）。
    entry.builtin = find_provider_preset(&entry.id).is_some();
    Ok(Json(entry))
}

async fn delete_provider(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
) -> std::result::Result<Json<OkResponse>, ApiError> {
    let store = state
        .provider_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("供应商存储不可用"))?;
    let removed = store.delete(&id).map_err(ApiError::internal)?;
    if !removed {
        return Err(ApiError::not_found("自定义供应商不存在"));
    }
    // 删除的是内置预设的「覆盖」（恢复默认）时，保留其已保存的 API Key；
    // 只有彻底删除一个纯自定义供应商，才一并清除它的 Key。
    if find_provider_preset(&id).is_none() {
        if let Some(auth) = state.auth_store.as_ref() {
            let _ = auth.delete_provider_key(&id);
        }
    }
    Ok(Json(OkResponse { ok: true }))
}

async fn export_providers(
    State(state): State<IpcState>,
) -> std::result::Result<Json<Vec<CustomProvider>>, ApiError> {
    let store = state
        .provider_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("供应商存储不可用"))?;
    Ok(Json(store.export().map_err(ApiError::internal)?))
}

async fn import_providers(
    State(state): State<IpcState>,
    Json(req): Json<ImportProvidersRequest>,
) -> std::result::Result<Json<ImportProvidersResponse>, ApiError> {
    let store = state
        .provider_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("供应商存储不可用"))?;
    let imported = store
        .import(req.providers, req.replace)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(ImportProvidersResponse { imported }))
}

async fn set_provider_api_key(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ApiKeyLoginRequest>,
) -> std::result::Result<Json<OkResponse>, ApiError> {
    if req.api_key.trim().is_empty() {
        return Err(ApiError::bad_request("API Key 不能为空"));
    }
    let auth = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    auth.set_provider_key(&id, &req.api_key)
        .map_err(ApiError::internal)?;
    Ok(Json(OkResponse { ok: true }))
}

/// 自动获取某供应商可用模型：调上游 OpenAI 兼容 `GET {base_url}/models`，
/// 失败 / 无 Key / 不兼容时回退内置目录（`entry_models`）。
async fn fetch_provider_models(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
) -> std::result::Result<Json<ProviderModelsResponse>, ApiError> {
    let entry = resolve_entry(&state, &id)?
        .ok_or_else(|| ApiError::not_found("供应商不存在"))?;
    let catalog = entry_models(&entry);
    // Key：已保存的供应商 Key > 环境变量。
    let key = state
        .auth_store
        .as_ref()
        .and_then(|s| s.get_provider_key(&id).ok().flatten())
        .or_else(|| entry.api_key_from_env());

    let url = format!("{}/models", entry.base_url.trim_end_matches('/'));
    let mut builder = state
        .http_client
        .get(&url)
        .timeout(std::time::Duration::from_secs(12));
    if let Some(k) = key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        builder = builder.bearer_auth(k);
    }
    match fetch_models_from_upstream(builder).await {
        Ok(models) if !models.is_empty() => Ok(Json(ProviderModelsResponse {
            models,
            source: "upstream".into(),
            error: None,
        })),
        Ok(_) => Ok(Json(ProviderModelsResponse {
            models: catalog,
            source: "catalog".into(),
            error: Some("上游未返回模型，已回退内置目录".into()),
        })),
        Err(e) => Ok(Json(ProviderModelsResponse {
            models: catalog,
            source: "catalog".into(),
            error: Some(format!("获取上游模型失败（{e}），已回退内置目录")),
        })),
    }
}

/// 解析 OpenAI 兼容 `/models` 响应里的模型 id 列表（去重保序）。
async fn fetch_models_from_upstream(builder: reqwest::RequestBuilder) -> anyhow::Result<Vec<String>> {
    let resp = builder.send().await?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {}", status.as_u16());
    }
    let body: serde_json::Value = resp.json().await?;
    let mut out: Vec<String> = Vec::new();
    if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(mid) = item.get("id").and_then(|x| x.as_str()) {
                let mid = mid.trim();
                if !mid.is_empty() && !out.iter().any(|x| x == mid) {
                    out.push(mid.to_string());
                }
            }
        }
    }
    Ok(out)
}

async fn list_accounts(
    State(state): State<IpcState>,
) -> std::result::Result<Json<Vec<AccountSummary>>, ApiError> {
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let accounts = store.list().map_err(ApiError::internal)?;
    // 读本机 `~/.codex/auth.json` 身份，标记「当前」账号（Codex 实际在用的那个）。
    let local = local_codex_identity(&state);
    // 账号元数据（名称/标签/备注）与按账号 token 用量，一次读取后逐个合并。
    let meta_map = store.account_meta_all().unwrap_or_default();
    let usage_map = account_usage_map(&state);
    let summaries = accounts
        .iter()
        .map(|a| {
            let mut summary = AccountSummary::from(a);
            let sid = summary.id.clone();
            apply_provider_display_name(&state, a, &mut summary);
            apply_account_meta(&mut summary, meta_map.get(&sid));
            apply_account_usage(&mut summary, usage_map.get(&sid));
            if let Some((account_id, email)) = &local {
                let id_match = a.account_id.is_some() && a.account_id == *account_id;
                let email_match = a.email.is_some() && a.email == *email;
                summary.current = id_match || email_match;
            }
            summary
        })
        .collect();
    Ok(Json(summaries))
}

/// 读取「按账号聚合的 token 用量」映射（会话存储不可用时返回空表）。
fn account_usage_map(state: &IpcState) -> HashMap<String, AccountUsage> {
    let Some(store) = state.session_store.as_ref() else {
        return HashMap::new();
    };
    let Ok(guard) = store.lock() else {
        return HashMap::new();
    };
    guard
        .account_usage()
        .unwrap_or_default()
        .into_iter()
        .map(|u| (u.account.clone(), u))
        .collect()
}

/// 合并账号元数据：标签 / 备注，并在有自定义名时覆盖展示名。
fn apply_account_meta(summary: &mut AccountSummary, meta: Option<&AccountMeta>) {
    let Some(meta) = meta else {
        return;
    };
    summary.tags = meta.tags.clone();
    summary.note = meta.note.clone();
    summary.model = meta.model.clone();
    if let Some(label) = meta.label.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        summary.label = Some(label.to_string());
        summary.display_name = label.to_string();
    }
}

/// 合并该账号的累计 token 用量与请求数。
fn apply_account_usage(summary: &mut AccountSummary, usage: Option<&AccountUsage>) {
    let Some(u) = usage else {
        return;
    };
    summary.tokens_used = u.total_tokens;
    summary.input_tokens = u.input_tokens;
    summary.output_tokens = u.output_tokens;
    summary.requests = u.requests;
}

/// 读取本机 Codex 凭据（`~/.codex/auth.json`）的身份（account_id, email），best-effort。
fn local_codex_identity(state: &IpcState) -> Option<(Option<String>, Option<String>)> {
    let cfg = state.codex_config.as_ref()?;
    let path = cfg.home().join("auth.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let account = parse_codex_cli_auth(&raw).ok()?;
    Some((account.account_id, account.email))
}

async fn add_api_key_account(
    State(state): State<IpcState>,
    Json(req): Json<ApiKeyLoginRequest>,
) -> std::result::Result<Json<AccountSummary>, ApiError> {
    if req.api_key.trim().is_empty() {
        return Err(ApiError::bad_request("API Key 不能为空"));
    }
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    // 归属供应商：非空且非 `codex` 时，账号记到该供应商，并把 Key 绑定给供应商，
    // 供代理在启用该供应商时取用（与供应商页「设置 API Key」同一份数据源）。
    let provider = req
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "codex");
    let account = match provider {
        Some(pid) => {
            let account =
                login_with_api_key_for(store, &req.api_key, pid).map_err(ApiError::internal)?;
            store
                .set_provider_key(pid, &req.api_key)
                .map_err(ApiError::internal)?;
            account
        }
        None => login_with_api_key(store, &req.api_key).map_err(ApiError::internal)?,
    };
    record_account_event(&state, "added", &account);
    Ok(Json(account_summary(&state, &account)))
}

/// 构造账号摘要，并把无 email/account_id 的供应商 API Key 账号的展示名
/// 解析为供应商友好名（否则会显示「未知账号」）。
fn account_summary(state: &IpcState, account: &Account) -> AccountSummary {
    let mut summary = AccountSummary::from(account);
    apply_provider_display_name(state, account, &mut summary);
    summary
}

/// 当账号没有 email/account_id（展示名回退为「未知账号」）时，
/// 用供应商友好名替代，便于在账号页区分不同供应商的 API Key 账号。
fn apply_provider_display_name(state: &IpcState, account: &Account, summary: &mut AccountSummary) {
    if summary.display_name != "未知账号" {
        return;
    }
    if account.provider.is_empty() || account.provider == "codex" {
        return;
    }
    let name = resolve_entry(state, &account.provider)
        .ok()
        .flatten()
        .map(|e| e.name)
        .unwrap_or_else(|| account.provider.clone());
    summary.display_name = name;
}

/// 记录账号增删事件到会话存储（用于仪表盘统计），best-effort。
fn record_account_event(state: &IpcState, event: &str, account: &Account) {
    let Some(store) = state.session_store.as_ref() else {
        return;
    };
    let Ok(guard) = store.lock() else {
        return;
    };
    let key = account_id(account);
    let mode = format!("{:?}", account.auth_mode).to_lowercase();
    if let Err(e) = guard.record_account_event(event, &key, &mode) {
        tracing::warn!("记录账号事件失败: {e}");
    }
}

async fn codex_login(
    State(state): State<IpcState>,
    Json(req): Json<CodexLoginRequest>,
) -> std::result::Result<Json<AccountSummary>, ApiError> {
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let config = OAuthConfig::default();
    let account = login_with_browser(&config, store, req.open_browser.unwrap_or(true))
        .await
        .map_err(ApiError::internal)?;
    record_account_event(&state, "added", &account);
    Ok(Json(AccountSummary::from(&account)))
}

#[derive(Debug, Deserialize)]
pub struct CodexLoginRequest {
    pub open_browser: Option<bool>,
}

async fn add_codex_token_account(
    State(state): State<IpcState>,
    Json(req): Json<CodexTokenRequest>,
) -> std::result::Result<Json<AccountSummary>, ApiError> {
    if req.id_token.trim().is_empty() && req.access_token.trim().is_empty() {
        return Err(ApiError::bad_request("id_token 与 access_token 不能同时为空"));
    }
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let refresh = req.refresh_token.filter(|t| !t.trim().is_empty());
    let account = login_with_tokens(store, req.id_token, req.access_token, refresh)
        .map_err(ApiError::internal)?;
    record_account_event(&state, "added", &account);
    Ok(Json(AccountSummary::from(&account)))
}

#[derive(Debug, Deserialize)]
pub struct CodexTokenRequest {
    #[serde(default)]
    pub id_token: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

async fn import_codex_local_account(
    State(state): State<IpcState>,
) -> std::result::Result<Json<AccountSummary>, ApiError> {
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    let path = cfg.home().join("auth.json");
    if !path.exists() {
        return Err(ApiError::not_found(format!(
            "未找到本机 Codex 凭据：{}",
            path.display()
        )));
    }
    let account = import_codex_account(store, &path).map_err(ApiError::internal)?;
    record_account_event(&state, "added", &account);
    Ok(Json(AccountSummary::from(&account)))
}

async fn import_codex_json_accounts(
    State(state): State<IpcState>,
    Json(req): Json<ImportJsonRequest>,
) -> std::result::Result<Json<ImportAccountsResponse>, ApiError> {
    if req.content.trim().is_empty() {
        return Err(ApiError::bad_request("JSON 内容不能为空"));
    }
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    // 解析失败属用户输入问题，返回 400。
    let accounts = import_codex_from_json(store, &req.content)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    for account in &accounts {
        record_account_event(&state, "added", account);
    }
    Ok(Json(ImportAccountsResponse {
        imported: accounts.len(),
        accounts: accounts.iter().map(AccountSummary::from).collect(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct ImportJsonRequest {
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ImportAccountsResponse {
    pub imported: usize,
    pub accounts: Vec<AccountSummary>,
}

async fn delete_account(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
) -> std::result::Result<Json<OkResponse>, ApiError> {
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let accounts = store.list().map_err(ApiError::internal)?;
    let account = accounts
        .iter()
        .find(|a| account_id(a) == id)
        .ok_or_else(|| ApiError::not_found("账号不存在"))?;
    store.delete(account).map_err(ApiError::internal)?;
    // 顺带清理该账号的元数据（名称/标签/备注）。
    let _ = store.delete_account_meta(&id);
    record_account_event(&state, "deleted", account);
    Ok(Json(OkResponse { ok: true }))
}

/// 编辑账号：自定义名称 / 标签 / 备注；API Key 账号可更新 Key。
async fn update_account(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UpdateAccountRequest>,
) -> std::result::Result<Json<AccountSummary>, ApiError> {
    let store = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let accounts = store.list().map_err(ApiError::internal)?;
    let account = accounts
        .iter()
        .find(|a| account_id(a) == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("账号不存在"))?;

    // 合并元数据（仅覆盖请求里出现的字段；label/note 传空串表示清除）。
    let mut meta = store.get_account_meta(&id).unwrap_or_default();
    if let Some(label) = req.label {
        meta.label = Some(label).filter(|s| !s.trim().is_empty());
    }
    if let Some(note) = req.note {
        meta.note = Some(note).filter(|s| !s.trim().is_empty());
    }
    if let Some(tags) = req.tags {
        meta.tags = tags;
    }
    if let Some(model) = req.model {
        meta.model = Some(model).filter(|s| !s.trim().is_empty());
    }
    store
        .set_account_meta(&id, meta)
        .map_err(ApiError::internal)?;

    // 更新 API Key（仅 API Key 账号）。
    if let Some(key) = req.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        if account.auth_mode != AuthMode::ApiKey {
            return Err(ApiError::bad_request("仅 API Key 账号可更新 Key"));
        }
        let mut full = store.load_secret(&account).unwrap_or_else(|_| account.clone());
        full.api_key = Some(key.to_string());
        store.save(&full).map_err(ApiError::internal)?;
        // 绑定到具体供应商的账号：同步更新供应商 Key，供代理取用。
        if !account.provider.is_empty() && account.provider != "codex" {
            store
                .set_provider_key(&account.provider, key)
                .map_err(ApiError::internal)?;
        }
    }

    // 返回合并后的最新摘要。
    let refreshed = store
        .list()
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|a| account_id(a) == id)
        .unwrap_or(account);
    let mut summary = AccountSummary::from(&refreshed);
    apply_provider_display_name(&state, &refreshed, &mut summary);
    apply_account_meta(&mut summary, store.get_account_meta(&id).ok().as_ref());
    apply_account_usage(&mut summary, account_usage_map(&state).get(&id));
    Ok(Json(summary))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAccountRequest {
    /// 自定义名称（传空串清除，省略则不改）。
    #[serde(default)]
    pub label: Option<String>,
    /// 标签（提供则整组替换，省略则不改）。
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// 备注（传空串清除，省略则不改）。
    #[serde(default)]
    pub note: Option<String>,
    /// 新 API Key（仅 API Key 账号，省略或空则不改）。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 账号偏好模型（传空串清除，省略则不改）。
    #[serde(default)]
    pub model: Option<String>,
}

async fn list_sessions(
    State(state): State<IpcState>,
    Query(q): Query<ListSessionsQuery>,
) -> std::result::Result<Json<Vec<ferry_codexlog::SessionRecord>>, ApiError> {
    // 会话与 token 统一来自 Codex 本地 rollout（换号后历史仍在；OAuth 直连也能统计）。
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as usize;
    let mut sessions = ferry_codexlog::CodexLog::with_home(cfg.home()).list_sessions();
    sessions.truncate(limit);
    Ok(Json(sessions))
}

async fn active_provider(
    State(state): State<IpcState>,
) -> std::result::Result<Json<ActiveProviderResponse>, ApiError> {
    let provider = state
        .active_provider
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("运行时供应商状态不可用"))?;
    let provider = provider
        .read()
        .map_err(|e| ApiError::internal(anyhow::anyhow!("供应商状态锁已损坏: {e}")))?;
    Ok(Json(ActiveProviderResponse::from_config(None, &provider)))
}

async fn switch_provider(
    State(state): State<IpcState>,
    Json(req): Json<SwitchProviderRequest>,
) -> std::result::Result<Json<ActiveProviderResponse>, ApiError> {
    let entry = resolve_entry(&state, &req.provider_id)?
        .ok_or_else(|| ApiError::not_found("供应商不存在"))?;
    // 指定账号时：取该账号的稳定 id（按账号统计 token）与其保存的 Key。
    let mut override_key = req.api_key.clone();
    let mut account_key = String::new();
    if let Some(acc_id) = req
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        account_key = acc_id.to_string();
        let has_override = override_key
            .as_deref()
            .map(str::trim)
            .is_some_and(|k| !k.is_empty());
        if !has_override {
            if let Some(auth) = state.auth_store.as_ref() {
                if let Some(found) = auth
                    .list()
                    .ok()
                    .and_then(|list| list.into_iter().find(|a| account_id(a) == acc_id))
                {
                    if let Ok(full) = auth.load_secret(&found) {
                        override_key = full.api_key.filter(|k| !k.trim().is_empty());
                    }
                }
            }
        }
    }
    // 显式传入 / 账号取出的 key 持久化保存，供后续复用。
    if let Some(key) = override_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        if let Some(auth) = state.auth_store.as_ref() {
            auth.set_provider_key(&entry.id, key)
                .map_err(ApiError::internal)?;
        }
    }
    let next = build_provider_config(&entry, state.auth_store.as_ref(), override_key, account_key);
    let provider = state
        .active_provider
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("运行时供应商状态不可用"))?;
    {
        let mut guard = provider
            .write()
            .map_err(|e| ApiError::internal(anyhow::anyhow!("供应商状态锁已损坏: {e}")))?;
        *guard = next;
    }
    let current = provider
        .read()
        .map_err(|e| ApiError::internal(anyhow::anyhow!("供应商状态锁已损坏: {e}")))?;
    Ok(Json(ActiveProviderResponse::from_config(
        Some(entry.id.clone()),
        &current,
    )))
}

/// 统一「使用账号」入参：可选覆盖该账号偏好模型。
#[derive(Debug, Default, Deserialize)]
pub struct UseAccountRequest {
    #[serde(default)]
    pub model: Option<String>,
}

/// 统一「使用账号」响应。`mode`：`direct`（OAuth 直连官方）/ `proxy`（中转/供应商经本地代理）。
#[derive(Debug, Serialize)]
pub struct UseAccountResponse {
    pub ok: bool,
    pub mode: String,
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
}

/// 统一「使用账号」：按账号类型**自动识别路由**，无需用户手选模式。
///
/// - **OAuth(ChatGPT)**：写 `~/.codex/auth.json`（Codex CLI 格式）让 Codex 直连官方，
///   并 `release` 撤销码渡注入的代理 provider（不经 ferry-proxy）。
/// - **API Key / 中转 / 供应商**：保存 Key、写运行时供应商、`takeover` 把 config.toml
///   指向本地代理（`http://127.0.0.1:15721/v1`），由 ferry-proxy 做 Responses↔Chat 转换。
async fn use_account(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UseAccountRequest>,
) -> std::result::Result<Json<UseAccountResponse>, ApiError> {
    let auth = state
        .auth_store
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号存储不可用"))?;
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;

    let summary = auth
        .list()
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|a| account_id(a) == id)
        .ok_or_else(|| ApiError::not_found("账号不存在"))?;
    let full = auth.load_secret(&summary).map_err(ApiError::internal)?;

    // 模型优先级：请求显式 > 账号偏好（meta.model）。
    let meta_model = auth.get_account_meta(&id).ok().and_then(|m| m.model);
    let model = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| m.to_string())
        .or(meta_model);

    match full.auth_mode {
        AuthMode::Chatgpt => {
            let auth_json = serde_json::to_string_pretty(&full.to_codex_cli_auth_json())
                .map_err(|e| ApiError::internal(anyhow::anyhow!("序列化 auth.json 失败: {e}")))?;
            cfg.write_auth(&auth_json).map_err(ApiError::internal)?;
            cfg.release(DEFAULT_PROVIDER_KEY).map_err(ApiError::internal)?;
            if let Some(m) = model.as_deref() {
                let prefs = CodexPreferences {
                    model: Some(m.to_string()),
                    ..Default::default()
                };
                cfg.set_preferences(&prefs).map_err(ApiError::internal)?;
            }
            Ok(Json(UseAccountResponse {
                ok: true,
                mode: "direct".to_string(),
                account_id: id,
                model,
                config_path: Some(cfg.config_path().display().to_string()),
            }))
        }
        AuthMode::ApiKey => {
            let provider_id = full.provider.trim().to_string();
            if provider_id.is_empty() || provider_id == GENERIC_CODEX_PROVIDER {
                return Err(ApiError::bad_request(
                    "通用 API Key 账号请先绑定具体供应商再使用",
                ));
            }
            let default_model = apply_provider_switch(&state, &provider_id, &id, None)?;
            let params = TakeoverParams {
                model: model.clone(),
                ..Default::default()
            };
            cfg.takeover(&params).map_err(ApiError::internal)?;
            Ok(Json(UseAccountResponse {
                ok: true,
                mode: "proxy".to_string(),
                account_id: id,
                model: model.or(Some(default_model)),
                config_path: Some(cfg.config_path().display().to_string()),
            }))
        }
    }
}

/// 通用（未绑定具体供应商）API Key 账号的 provider 占位值。
const GENERIC_CODEX_PROVIDER: &str = "codex";

/// 切换运行时供应商（保存账号 Key + 写 `active_provider`），返回该供应商默认模型。
///
/// 供 [`use_account`] 复用 [`switch_provider`] 的核心逻辑。
fn apply_provider_switch(
    state: &IpcState,
    provider_id: &str,
    account_key: &str,
    override_key: Option<String>,
) -> std::result::Result<String, ApiError> {
    let entry =
        resolve_entry(state, provider_id)?.ok_or_else(|| ApiError::not_found("供应商不存在"))?;
    let account_key = account_key.trim().to_string();
    let mut override_key = override_key.filter(|k| !k.trim().is_empty());
    if override_key.is_none() && !account_key.is_empty() {
        if let Some(auth) = state.auth_store.as_ref() {
            if let Some(found) = auth
                .list()
                .ok()
                .and_then(|list| list.into_iter().find(|a| account_id(a) == account_key))
            {
                if let Ok(full) = auth.load_secret(&found) {
                    override_key = full.api_key.filter(|k| !k.trim().is_empty());
                }
            }
        }
    }
    if let Some(key) = override_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        if let Some(auth) = state.auth_store.as_ref() {
            auth.set_provider_key(&entry.id, key)
                .map_err(ApiError::internal)?;
        }
    }
    let next = build_provider_config(&entry, state.auth_store.as_ref(), override_key, account_key);
    let default_model = next.default_model.clone();
    let provider = state
        .active_provider
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("运行时供应商状态不可用"))?;
    {
        let mut guard = provider
            .write()
            .map_err(|e| ApiError::internal(anyhow::anyhow!("供应商状态锁已损坏: {e}")))?;
        *guard = next;
    }
    Ok(default_model)
}

fn resolve_entry(
    state: &IpcState,
    id: &str,
) -> std::result::Result<Option<ProviderEntry>, ApiError> {
    match state.provider_store.as_ref() {
        Some(store) => resolve_provider(store, id).map_err(ApiError::internal),
        None => Ok(find_provider_preset(id).map(ProviderEntry::from_preset)),
    }
}

// ===================== 路由模式 & 账号池 =====================

fn current_mode(state: &IpcState) -> RouteMode {
    state
        .route_mode
        .as_ref()
        .and_then(|m| m.read().ok().map(|g| *g))
        .unwrap_or(RouteMode::Provider)
}

fn pool_cell(state: &IpcState) -> std::result::Result<&Arc<RwLock<AccountPool>>, ApiError> {
    state
        .account_pool
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("账号池不可用"))
}

fn pool_snapshot(state: &IpcState) -> std::result::Result<PoolResponse, ApiError> {
    let snapshot = pool_cell(state)?
        .read()
        .map_err(|e| ApiError::internal(anyhow::anyhow!("账号池锁已损坏: {e}")))?
        .snapshot();
    Ok(PoolResponse {
        mode: current_mode(state).as_str().to_string(),
        snapshot,
    })
}

async fn get_pool(
    State(state): State<IpcState>,
) -> std::result::Result<Json<PoolResponse>, ApiError> {
    Ok(Json(pool_snapshot(&state)?))
}

/// 从账号存储加载 Codex 账号到账号池（仅纳入含 access_token 的 ChatGPT 账号）。
pub fn load_pool_accounts(auth: &AuthStore) -> Vec<PoolAccount> {
    let accounts = match auth.list() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("读取账号列表以装配账号池失败: {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for acc in accounts {
        let full = auth.load_secret(&acc).unwrap_or_else(|_| acc.clone());
        let access_token = full
            .tokens
            .as_ref()
            .map(|t| t.access_token.clone())
            .unwrap_or_default();
        if access_token.trim().is_empty() {
            continue; // 纯 API Key 账号不进 Codex 账号池
        }
        out.push(PoolAccount {
            key: account_id(&full),
            display_name: full.display_name(),
            base_url: CODEX_BACKEND_BASE_URL.to_string(),
            access_token,
            account_id: full.account_id.clone(),
            auth_mode: format!("{:?}", full.auth_mode).to_lowercase(),
            expires_at: full.expires_at.map(|t| t.timestamp()),
        });
    }
    out
}

async fn get_session(
    State(state): State<IpcState>,
    AxumPath(id): AxumPath<String>,
) -> std::result::Result<Json<ferry_codexlog::SessionDetail>, ApiError> {
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    let detail = ferry_codexlog::CodexLog::with_home(cfg.home())
        .read_session_detail(&id)
        .ok_or_else(|| ApiError::not_found("会话不存在"))?;
    Ok(Json(detail))
}

async fn stats(
    State(state): State<IpcState>,
    Query(q): Query<StatsQuery>,
) -> std::result::Result<Json<StatsResponse>, ApiError> {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let now = Utc::now();
    let today = now.date_naive();
    let start = today - Duration::days(days - 1);
    let since_day = start.format("%Y-%m-%d").to_string();

    // 账号快照（当前数量 / 失效数）。
    let accounts = state
        .auth_store
        .as_ref()
        .map(|s| s.list().unwrap_or_default())
        .unwrap_or_default();
    let accounts_current = accounts.len() as i64;
    let accounts_expired = accounts
        .iter()
        .filter(|a| a.expires_at.map(|e| e < now).unwrap_or(false))
        .count() as i64;

    let mut totals = StatsTotals {
        accounts_current,
        accounts_expired,
        ..Default::default()
    };
    let mut token_map: HashMap<String, DayTokens> = HashMap::new();
    let mut account_map: HashMap<String, DayAccounts> = HashMap::new();
    let mut usage_rows: Vec<ProviderUsage> = Vec::new();

    if let Some(store_arc) = state.session_store.as_ref() {
        if let Ok(store) = store_arc.lock() {
            // 存量回填：当前存在但无 added 事件的账号补一条（幂等）。
            if let Ok(existing) = store.account_added_keys() {
                for a in &accounts {
                    let key = account_id(a);
                    if !existing.contains(&key) {
                        let ts = a.last_refresh.unwrap_or(now).to_rfc3339();
                        let mode = format!("{:?}", a.auth_mode).to_lowercase();
                        let _ = store.record_account_event_at("added", &key, &mode, &ts);
                    }
                }
            }

            let st = store.session_totals().unwrap_or_default();
            totals.total_tokens = st.total_tokens;
            totals.input_tokens = st.input_tokens;
            totals.output_tokens = st.output_tokens;
            totals.requests = st.requests;
            totals.succeeded = st.succeeded;
            totals.failed = st.failed;

            let (added, deleted) = store.account_event_totals().unwrap_or((0, 0));
            totals.accounts_added = added;
            totals.accounts_deleted = deleted;

            for d in store.daily_token_series(&since_day).unwrap_or_default() {
                token_map.insert(d.date.clone(), d);
            }
            for d in store.daily_account_series(&since_day).unwrap_or_default() {
                account_map.insert(d.date.clone(), d);
            }
            usage_rows = store.provider_usage().unwrap_or_default();
        }
    }

    // rollout 真实用量优先：Codex 自己写的会话文件（OAuth 直连 / 供应商经代理都会写），
    // 比代理层 tiktoken 估算更准。同时按会话更新日期聚合每日 token，让「今日 / 每日 token」
    // 在方案 A（OAuth 直连、不经 SQLite 代理）下也能更新。无 rollout 时保留上面的 SQLite 数据。
    let mut rollout_daily: HashMap<String, (i64, i64, i64)> = HashMap::new();
    if let Some(cfg) = state.codex_config.as_ref() {
        let sessions = ferry_codexlog::CodexLog::with_home(cfg.home()).list_sessions();
        if !sessions.is_empty() {
            let (mut sum_in, mut sum_out, mut sum_total) = (0i64, 0i64, 0i64);
            for s in &sessions {
                let si = s.input_tokens as i64;
                let so = s.output_tokens as i64;
                let st = s.total_tokens as i64;
                sum_in += si;
                sum_out += so;
                sum_total += st;
                if let Some(ts) = s.updated_at {
                    if let Some(dt) = chrono::DateTime::from_timestamp(ts, 0) {
                        let key = dt.format("%Y-%m-%d").to_string();
                        let e = rollout_daily.entry(key).or_insert((0, 0, 0));
                        e.0 += si;
                        e.1 += so;
                        e.2 += st;
                    }
                }
            }
            totals.total_tokens = sum_total;
            totals.input_tokens = sum_in;
            totals.output_tokens = sum_out;
        }
    }

    let provider_usage: Vec<ProviderUsagePoint> =
        usage_rows.into_iter().map(ProviderUsagePoint::from_row).collect();

    // 连续日序列（缺失补零）。
    let mut series = Vec::new();
    let mut day = start;
    while day <= today {
        let date = day.format("%Y-%m-%d").to_string();
        let t = token_map.get(&date);
        let a = account_map.get(&date);
        // token 维度优先取 rollout 当日聚合（真实用量）；请求成败 / 账号变动仍来自 SQLite。
        let (tokens, input_tokens, output_tokens) = match rollout_daily.get(&date) {
            Some(&(i, o, tt)) => (tt, i, o),
            None => (
                t.map_or(0, |x| x.total_tokens),
                t.map_or(0, |x| x.input_tokens),
                t.map_or(0, |x| x.output_tokens),
            ),
        };
        series.push(DayPoint {
            date,
            tokens,
            input_tokens,
            output_tokens,
            requests: t.map_or(0, |x| x.requests),
            succeeded: t.map_or(0, |x| x.succeeded),
            failed: t.map_or(0, |x| x.failed),
            accounts_added: a.map_or(0, |x| x.added),
            accounts_deleted: a.map_or(0, |x| x.deleted),
        });
        day += Duration::days(1);
    }

    totals.success_rate = if totals.requests > 0 {
        totals.succeeded as f64 / totals.requests as f64
    } else {
        0.0
    };
    let alive_usable = (accounts_current - accounts_expired).max(0);
    totals.survival_rate = if totals.accounts_added > 0 {
        (alive_usable as f64 / totals.accounts_added as f64).clamp(0.0, 1.0)
    } else if accounts_current > 0 {
        1.0
    } else {
        0.0
    };

    Ok(Json(StatsResponse {
        days,
        generated_at: now.to_rfc3339(),
        totals,
        series,
        provider_usage,
    }))
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub days: Option<i64>,
}

#[derive(Debug, Default, Serialize)]
pub struct StatsTotals {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub requests: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub success_rate: f64,
    pub accounts_current: i64,
    pub accounts_added: i64,
    pub accounts_deleted: i64,
    pub accounts_expired: i64,
    pub survival_rate: f64,
}

#[derive(Debug, Serialize)]
pub struct DayPoint {
    pub date: String,
    pub tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub requests: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub accounts_added: i64,
    pub accounts_deleted: i64,
}

/// 单个供应商/账号的 token 用量对比（上游上报 vs 本地估算）。
#[derive(Debug, Serialize)]
pub struct ProviderUsagePoint {
    /// provider 标识（供应商 base_url，或 `codex-pool:<账号>`）。
    pub provider: String,
    /// 是否账号池(Codex 官方)记录——官方 usage 可信，不参与掺假判定。
    pub is_pool: bool,
    pub requests: i64,
    pub reported_total: i64,
    pub reported_input: i64,
    pub reported_output: i64,
    pub est_total: i64,
    pub est_input: i64,
    pub est_output: i64,
    /// 上报/估算 比值（est_total>0 时有值）。>1 表示上游报数高于本地估算。
    pub ratio: Option<f64>,
    /// 疑似掺假：上游上报显著高于本地独立估算（仅对有估算的第三方供应商判定）。
    pub suspect: bool,
}

impl ProviderUsagePoint {
    /// 掺假阈值：上报 > 估算 × 1.8 且绝对差 > 300 token（保守，避免分词差异误报）。
    const SUSPECT_RATIO: f64 = 1.8;
    const SUSPECT_MIN_DIFF: i64 = 300;

    fn from_row(u: ProviderUsage) -> Self {
        let is_pool = u.provider.starts_with("codex-pool:");
        let ratio = if u.est_total > 0 {
            Some(u.reported_total as f64 / u.est_total as f64)
        } else {
            None
        };
        let suspect = !is_pool
            && u.est_total > 0
            && (u.reported_total as f64) > (u.est_total as f64) * Self::SUSPECT_RATIO
            && (u.reported_total - u.est_total) > Self::SUSPECT_MIN_DIFF;
        Self {
            provider: u.provider,
            is_pool,
            requests: u.requests,
            reported_total: u.reported_total,
            reported_input: u.reported_input,
            reported_output: u.reported_output,
            est_total: u.est_total,
            est_input: u.est_input,
            est_output: u.est_output,
            ratio,
            suspect,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub days: i64,
    pub generated_at: String,
    pub totals: StatsTotals,
    pub series: Vec<DayPoint>,
    /// 按供应商/账号的 token 用量对比（上报 vs 本地估算，含掺假标记）。
    pub provider_usage: Vec<ProviderUsagePoint>,
}

async fn codex_status(
    State(state): State<IpcState>,
) -> std::result::Result<Json<CodexStatusResponse>, ApiError> {
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    // 以 ~/.codex 真实文件为准（重启后状态也正确，不依赖 daemon 内存）：
    // - config.toml 含 [model_providers.codexferry] => 供应商代理接管（proxy）
    // - 否则 auth.json 有 OAuth 账号 => 账号直连官方（direct）
    // - 都没有 => 未接管（none）
    let raw = cfg.read_raw().unwrap_or_default();
    let managed = raw.contains(&format!("[model_providers.{}]", DEFAULT_PROVIDER_KEY));
    let model = cfg
        .read_preferences()
        .ok()
        .and_then(|p| p.model)
        .filter(|m| !m.trim().is_empty());
    let auth_text = cfg.read_auth().unwrap_or_default();
    let auth_account_id = parse_auth_account_id(&auth_text);
    let has_oauth = auth_account_id.is_some()
        || auth_text.contains("access_token")
        || auth_text.contains("refresh_token");
    let mode = if managed {
        "proxy"
    } else if has_oauth {
        "direct"
    } else {
        "none"
    };
    Ok(Json(CodexStatusResponse {
        home: cfg.home().display().to_string(),
        config_path: cfg.config_path().display().to_string(),
        exists: cfg.exists(),
        managed,
        mode: mode.to_string(),
        model,
        auth_account_id,
    }))
}

/// 从 auth.json 文本解析当前 OAuth 账号 account_id（Codex CLI 嵌套 `tokens{}` 或扁平格式）。
fn parse_auth_account_id(auth_text: &str) -> Option<String> {
    if auth_text.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(auth_text).ok()?;
    v.pointer("/tokens/account_id")
        .or_else(|| v.get("account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

async fn codex_takeover(
    State(state): State<IpcState>,
    Json(params): Json<TakeoverParams>,
) -> std::result::Result<Json<TakeoverResponse>, ApiError> {
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    let report = cfg.takeover(&params).map_err(ApiError::internal)?;
    Ok(Json(TakeoverResponse {
        config_path: report.config_path.display().to_string(),
        backup_path: report.backup_path.map(|p| p.display().to_string()),
        provider_key: report.provider_key,
    }))
}

async fn codex_release(
    State(state): State<IpcState>,
    Json(req): Json<ReleaseRequest>,
) -> std::result::Result<Json<OkResponse>, ApiError> {
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    let key = req.provider_key.as_deref().unwrap_or(DEFAULT_PROVIDER_KEY);
    cfg.release(key).map_err(ApiError::internal)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn codex_restore_latest(
    State(state): State<IpcState>,
) -> std::result::Result<Json<RestoreLatestResponse>, ApiError> {
    let cfg = state
        .codex_config
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Codex 配置不可用"))?;
    let restored = cfg.restore_latest().map_err(ApiError::internal)?;
    Ok(Json(RestoreLatestResponse { restored }))
}

/// 解析运行时供应商配置。Key 优先级：显式覆盖 > 已保存(Keychain/文件) > 环境变量。
///
/// `account_key` 为当前承载请求的账号稳定 id（用于按账号统计 token；无则空串）。
fn build_provider_config(
    entry: &ProviderEntry,
    auth_store: Option<&AuthStore>,
    override_key: Option<String>,
    account_key: String,
) -> ProviderConfig {
    let api_key = override_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| auth_store.and_then(|s| s.get_provider_key(&entry.id).ok().flatten()))
        .or_else(|| entry.api_key_from_env())
        .unwrap_or_default();
    let model_map = entry
        .aliases
        .iter()
        .map(|alias| (alias.from.clone(), alias.to.clone()))
        .collect::<HashMap<_, _>>();

    ProviderConfig {
        base_url: entry.base_url.clone(),
        api_key,
        api_type: provider_api_to_upstream(entry.api),
        default_model: entry.default_model.clone(),
        model_map,
        account_key,
    }
}

fn provider_api_to_upstream(api: ProviderApi) -> UpstreamApi {
    match api {
        ProviderApi::Chat => UpstreamApi::Chat,
        ProviderApi::Responses => UpstreamApi::Responses,
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct ImportProvidersRequest {
    pub providers: Vec<CustomProvider>,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportProvidersResponse {
    pub imported: usize,
}

#[derive(Debug, Serialize)]
pub struct AccountSummary {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub auth_mode: String,
    pub last_refresh: Option<String>,
    pub expires_at: Option<String>,
    pub stored_in_keychain: bool,
    /// 是否为本机 Codex 当前在用的账号（由 `~/.codex/auth.json` 身份比对得出）。
    #[serde(default)]
    pub current: bool,
    /// 用户自定义名称（非空时前端用作展示名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 标签。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 备注。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 账号偏好模型（中转/厂商账号选定的真实模型名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// ChatGPT 订阅方案（从 id_token 的 `chatgpt_plan_type` 解析，
    /// 如 plus/pro/team/enterprise/business 等）。前端据此显示 plan 徽章。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// 该账号累计 token 用量（成功会话；用于卡片悬浮展示）。
    #[serde(default)]
    pub tokens_used: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    /// 该账号累计请求数（成功会话）。
    #[serde(default)]
    pub requests: i64,
}

#[derive(Debug, Deserialize)]
pub struct SwitchProviderRequest {
    pub provider_id: String,
    /// 可选：本次切换显式指定的 API Key（会被保存以便复用）。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 可选：本次使用的账号稳定 id。提供时优先取该账号保存的 Key，
    /// 并把该账号记入会话（按账号统计 token 用量）。
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActiveProviderResponse {
    pub provider_id: Option<String>,
    pub base_url: String,
    pub api_type: String,
    pub default_model: String,
    pub api_key_configured: bool,
}

impl ActiveProviderResponse {
    fn from_config(provider_id: Option<String>, config: &ProviderConfig) -> Self {
        Self {
            provider_id,
            base_url: config.base_url.clone(),
            api_type: format!("{:?}", config.api_type).to_lowercase(),
            default_model: config.default_model.clone(),
            api_key_configured: !config.api_key.is_empty(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PoolResponse {
    /// 当前路由模式（pool 时账号池才会真正参与转发）。
    pub mode: String,
    pub snapshot: PoolSnapshot,
}

#[derive(Debug, Deserialize)]
pub struct StrategyRequest {
    /// round_robin / quota_aware。
    pub strategy: String,
}

#[derive(Debug, Serialize)]
pub struct SettingsResponse {
    pub settings: AppSettings,
    /// apizero Key 是否已配置（不下发明文）。
    pub apizero_key_configured: bool,
}

#[derive(Debug, Deserialize)]
pub struct ApizeroKeyRequest {
    /// 留空表示清除已保存的 Key。
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct WeatherQuery {
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    /// 为 true 时跳过缓存强制刷新（前端手动「刷新天气」按钮用）。
    #[serde(default)]
    pub refresh: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeatherResponse {
    pub city: String,
    pub skycon: String,
    pub emoji: String,
    pub skycon_code: String,
    pub temperature: Option<f64>,
    pub apparent_temperature: Option<f64>,
    pub humidity_percent: Option<f64>,
    pub visibility_km: Option<f64>,
    pub wind_text: String,
    pub wind_level_text: String,
    pub aqi: Option<f64>,
    pub aqi_level: String,
    pub aqi_color: String,
    pub pm25: Option<f64>,
    pub forecast_keypoint: Option<String>,
    pub alerts: Vec<WeatherAlert>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeatherAlert {
    pub title: String,
    pub color: String,
    pub level: String,
}

#[derive(Debug, Deserialize)]
pub struct PoemRequest {
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PoemResponse {
    pub content: String,
    pub origin: String,
    pub author: String,
    pub category: String,
}

impl From<&Account> for AccountSummary {
    fn from(a: &Account) -> Self {
        Self {
            id: account_id(a),
            provider: a.provider.clone(),
            display_name: a.display_name(),
            email: a.email.clone(),
            account_id: a.account_id.clone(),
            auth_mode: format!("{:?}", a.auth_mode).to_lowercase(),
            last_refresh: a.last_refresh.map(|t| t.to_rfc3339()),
            expires_at: a.expires_at.map(|t| t.to_rfc3339()),
            stored_in_keychain: a.secret_ref.is_some(),
            current: false,
            label: None,
            tags: Vec::new(),
            note: None,
            model: None,
            plan: account_plan(a),
            tokens_used: 0,
            input_tokens: 0,
            output_tokens: 0,
            requests: 0,
        }
    }
}

/// 从账号 id_token 解析订阅方案（plan）。仅 OAuth 账号带 id_token；
/// API Key 账号没有方案信息，返回 None（前端按 API 徽章处理）。
fn account_plan(a: &Account) -> Option<String> {
    let id_token = a.tokens.as_ref().map(|t| t.id_token.as_str())?;
    if id_token.trim().is_empty() {
        return None;
    }
    ferry_auth::parse_id_token(id_token)
        .ok()
        .and_then(|c| c.plan_type)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

fn account_id(a: &Account) -> String {
    a.stable_id()
}

#[derive(Debug, Deserialize)]
pub struct ApiKeyLoginRequest {
    pub api_key: String,
    /// 可选：把该 API Key 归属到指定供应商（账号页选择的供应商 id）。
    /// 省略或为空 / `codex` 表示通用账号。供应商账号会同时把 Key 绑定给该供应商，
    /// 供代理在启用该供应商时取用。
    #[serde(default)]
    pub provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct CodexStatusResponse {
    pub home: String,
    pub config_path: String,
    pub exists: bool,
    /// config.toml 是否含码渡注入的 `[model_providers.codexferry]`（供应商代理接管）。
    pub managed: bool,
    /// 当前 Codex 实际模式：`proxy`(供应商代理) / `direct`(OAuth 账号直连官方) / `none`(未接管)。
    pub mode: String,
    /// config.toml 顶层当前生效模型（若有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 当前 `~/.codex/auth.json` 的 OAuth 账号 account_id（direct 模式）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_account_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TakeoverResponse {
    pub config_path: String,
    pub backup_path: Option<String>,
    pub provider_key: String,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseRequest {
    pub provider_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RestoreLatestResponse {
    pub restored: bool,
}

#[derive(Debug, Serialize)]
pub struct ProviderModelsResponse {
    /// 模型 id 列表（上游或回退目录）。
    pub models: Vec<String>,
    /// 来源：`upstream`（实时拉取）/ `catalog`（内置目录回退）。
    pub source: String,
    /// 回退时的说明（成功为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn internal(err: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": {
                "message": self.message,
                "type": "ferry_ipc_error",
            }
        });
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_config::CodexConfig;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "ferry-ipc-{tag}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn state_with(auth_dir: &Path, codex_home: Option<&Path>) -> IpcState {
        IpcState {
            auth_store: Some(AuthStore::with_dir(auth_dir)),
            codex_config: codex_home.map(CodexConfig::with_home),
            ..Default::default()
        }
    }

    #[test]
    fn builtin_providers_cover_mvp_vendors() {
        let providers = builtin_entries();
        let ids = providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, vec!["deepseek", "qwen", "kimi", "glm"]);
        assert!(providers
            .iter()
            .any(|p| p.aliases.iter().any(|a| a.from == "gpt-5-codex")));
    }

    #[test]
    fn account_summary_redacts_secret_material() {
        let account = Account {
            provider: "codex".into(),
            id: None,
            email: Some("u@example.com".into()),
            account_id: Some("acc".into()),
            auth_mode: ferry_auth::AuthMode::Chatgpt,
            tokens: Some(ferry_auth::StoredTokens {
                id_token: "id-secret".into(),
                access_token: "access-secret".into(),
                refresh_token: "refresh-secret".into(),
            }),
            api_key: Some("sk-secret".into()),
            last_refresh: None,
            expires_at: None,
            secret_ref: Some("codex-u@example.com".into()),
        };

        let json = serde_json::to_string(&AccountSummary::from(&account)).unwrap();
        assert!(json.contains("codex-u@example.com"));
        assert!(json.contains("u@example.com"));
        assert!(json.contains("stored_in_keychain"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));
        assert!(!json.contains("sk-secret"));
    }

    #[test]
    fn provider_usage_point_flags_relay_inflation() {
        // 中转上报远高于本地估算 → 疑似掺假。
        let relay = ProviderUsagePoint::from_row(ProviderUsage {
            provider: "https://relay.example.com/v1".to_string(),
            requests: 3,
            reported_total: 5000,
            reported_input: 2000,
            reported_output: 3000,
            est_total: 1000,
            est_input: 400,
            est_output: 600,
        });
        assert!(relay.suspect, "5000 vs 1000 应判定疑似掺假");
        assert!(relay.ratio.unwrap() > 1.8);
        assert!(!relay.is_pool);

        // 上报与估算接近 → 不判定。
        let honest = ProviderUsagePoint::from_row(ProviderUsage {
            provider: "https://api.deepseek.com/v1".to_string(),
            requests: 2,
            reported_total: 1100,
            reported_input: 500,
            reported_output: 600,
            est_total: 1000,
            est_input: 450,
            est_output: 550,
        });
        assert!(!honest.suspect);

        // 账号池(官方)即使无估算也不判定掺假。
        let pool = ProviderUsagePoint::from_row(ProviderUsage {
            provider: "codex-pool:alice".to_string(),
            requests: 1,
            reported_total: 9999,
            reported_input: 1,
            reported_output: 9998,
            est_total: 0,
            est_input: 0,
            est_output: 0,
        });
        assert!(!pool.suspect);
        assert!(pool.is_pool);
        assert!(pool.ratio.is_none());
    }

    /// codex_status 以 ~/.codex 真实文件为准（重启后状态也正确，不看 daemon 内存）：
    /// 空目录=未接管(none) / config.toml 含码渡 provider=proxy / auth.json 有 OAuth=direct。
    #[tokio::test]
    async fn codex_status_reflects_real_files_not_memory() {
        use std::fs;

        // 1) 空 home → 未接管。
        let none_dir = temp_dir("codex-status-none-auth");
        let none_home = temp_dir("codex-status-none-home");
        let st = codex_status(State(state_with(&none_dir, Some(&none_home))))
            .await
            .expect("空 home 也应返回状态")
            .0;
        assert_eq!(st.mode, "none");
        assert!(!st.managed);
        assert!(st.auth_account_id.is_none());

        // 2) config.toml 含码渡供应商代理 → proxy，顶层 model 读得出。
        let proxy_dir = temp_dir("codex-status-proxy-auth");
        let proxy_home = temp_dir("codex-status-proxy-home");
        fs::write(
            proxy_home.join("config.toml"),
            format!(
                "model = \"deepseek-chat\"\n\n[model_providers.{}]\nbase_url = \"http://127.0.0.1:15721/v1\"\nwire_api = \"responses\"\n",
                DEFAULT_PROVIDER_KEY
            ),
        )
        .unwrap();
        let st = codex_status(State(state_with(&proxy_dir, Some(&proxy_home))))
            .await
            .expect("proxy 状态")
            .0;
        assert_eq!(st.mode, "proxy");
        assert!(st.managed);
        assert_eq!(st.model.as_deref(), Some("deepseek-chat"));

        // 3) auth.json 有 OAuth tokens（无码渡 provider）→ direct，且解析出 account_id。
        let direct_dir = temp_dir("codex-status-direct-auth");
        let direct_home = temp_dir("codex-status-direct-home");
        fs::write(
            direct_home.join("auth.json"),
            r#"{"OPENAI_API_KEY":null,"tokens":{"id_token":"id","access_token":"ac","refresh_token":"rf","account_id":"acc-xyz"}}"#,
        )
        .unwrap();
        let st = codex_status(State(state_with(&direct_dir, Some(&direct_home))))
            .await
            .expect("direct 状态")
            .0;
        assert_eq!(st.mode, "direct");
        assert!(!st.managed);
        assert_eq!(st.auth_account_id.as_deref(), Some("acc-xyz"));
    }

    #[tokio::test]
    async fn api_key_account_bound_to_provider_lists_with_provider_name() {
        let dir = temp_dir("apikey-provider");
        let state = state_with(&dir, None);
        let req = ApiKeyLoginRequest {
            api_key: "sk-vendor-123".to_string(),
            provider_id: Some("deepseek".to_string()),
        };
        let summary = match add_api_key_account(State(state.clone()), Json(req)).await {
            Ok(Json(s)) => s,
            Err(e) => panic!("应成功创建供应商 API Key 账号：{}", e.message),
        };
        assert_eq!(summary.auth_mode, "apikey");
        assert_eq!(summary.provider, "deepseek");
        // 无 email/account_id 时，展示名应回退为供应商友好名而非「未知账号」。
        assert_eq!(summary.display_name, "DeepSeek");

        // Key 应同时绑定给供应商，供代理启用该供应商时取用。
        let auth = state.auth_store.as_ref().unwrap();
        assert_eq!(
            auth.get_provider_key("deepseek").unwrap().as_deref(),
            Some("sk-vendor-123")
        );

        // 账号列表里也应带供应商友好名出现。
        let accounts = match list_accounts(State(state)).await {
            Ok(Json(list)) => list,
            Err(e) => panic!("列表失败：{}", e.message),
        };
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].provider, "deepseek");
        assert_eq!(accounts[0].display_name, "DeepSeek");

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn api_key_account_generic_stays_codex() {
        let dir = temp_dir("apikey-generic");
        let state = state_with(&dir, None);
        let req = ApiKeyLoginRequest {
            api_key: "sk-generic-1".to_string(),
            provider_id: None,
        };
        let summary = match add_api_key_account(State(state.clone()), Json(req)).await {
            Ok(Json(s)) => s,
            Err(e) => panic!("应成功创建通用 API Key 账号：{}", e.message),
        };
        assert_eq!(summary.provider, "codex");
        // 通用账号不绑定到任何具体供应商。
        let auth = state.auth_store.as_ref().unwrap();
        assert!(auth.get_provider_key("deepseek").unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn multiple_api_key_accounts_same_provider_listed() {
        let dir = temp_dir("multi-apikey");
        let state = state_with(&dir, None);
        for key in ["sk-1", "sk-2"] {
            let _ = add_api_key_account(
                State(state.clone()),
                Json(ApiKeyLoginRequest {
                    api_key: key.to_string(),
                    provider_id: Some("deepseek".to_string()),
                }),
            )
            .await
            .expect("应能添加多个供应商账号");
        }
        let accounts = match list_accounts(State(state)).await {
            Ok(Json(list)) => list,
            Err(e) => panic!("列表失败：{}", e.message),
        };
        let ds: Vec<_> = accounts.iter().filter(|a| a.provider == "deepseek").collect();
        assert_eq!(ds.len(), 2, "同供应商两个 API Key 账号都应在列（不覆盖）");
        // id 应互不相同。
        assert_ne!(ds[0].id, ds[1].id);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn update_account_sets_label_tags_note() {
        let dir = temp_dir("update-acct");
        let state = state_with(&dir, None);
        let created = match add_api_key_account(
            State(state.clone()),
            Json(ApiKeyLoginRequest {
                api_key: "sk-orig".to_string(),
                provider_id: Some("deepseek".to_string()),
            }),
        )
        .await
        {
            Ok(Json(s)) => s,
            Err(e) => panic!("创建失败：{}", e.message),
        };
        let id = created.id.clone();

        let updated = match update_account(
            State(state.clone()),
            AxumPath(id.clone()),
            Json(UpdateAccountRequest {
                label: Some("工作号".to_string()),
                tags: Some(vec!["工作".to_string(), "高优".to_string()]),
                note: Some("主力 DeepSeek".to_string()),
                api_key: Some("sk-new".to_string()),
                model: Some("deepseek-reasoner".to_string()),
            }),
        )
        .await
        {
            Ok(Json(s)) => s,
            Err(e) => panic!("更新失败：{}", e.message),
        };
        assert_eq!(updated.display_name, "工作号", "自定义名应作展示名");
        assert_eq!(updated.tags, vec!["工作", "高优"]);
        assert_eq!(updated.note.as_deref(), Some("主力 DeepSeek"));

        // 列表应反映元数据；供应商 Key 应被同步为新 Key。
        let accounts = match list_accounts(State(state.clone())).await {
            Ok(Json(list)) => list,
            Err(e) => panic!("列表失败：{}", e.message),
        };
        let a = accounts.iter().find(|a| a.id == id).unwrap();
        assert_eq!(a.display_name, "工作号");
        assert_eq!(a.tags, vec!["工作", "高优"]);
        let auth = state.auth_store.as_ref().unwrap();
        assert_eq!(
            auth.get_provider_key("deepseek").unwrap().as_deref(),
            Some("sk-new"),
            "更新 Key 应同步给绑定供应商"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn codex_token_account_creates_oauth_account() {
        let dir = temp_dir("tok");
        let state = state_with(&dir, None);
        let req = CodexTokenRequest {
            id_token: "header.payload.sig".to_string(),
            access_token: "acc-token".to_string(),
            refresh_token: Some("ref-token".to_string()),
        };
        let summary = match add_codex_token_account(State(state), Json(req)).await {
            Ok(Json(s)) => s,
            Err(e) => panic!("应成功创建 token 账号：{}", e.message),
        };
        assert_eq!(summary.auth_mode, "chatgpt");
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn codex_token_account_rejects_empty() {
        let dir = temp_dir("tok-empty");
        let state = state_with(&dir, None);
        let req = CodexTokenRequest {
            id_token: "   ".to_string(),
            access_token: String::new(),
            refresh_token: None,
        };
        match add_codex_token_account(State(state), Json(req)).await {
            Ok(_) => panic!("空 token 应被拒绝"),
            Err(e) => assert_eq!(e.status, StatusCode::BAD_REQUEST),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn import_codex_local_imports_and_marks_current() {
        let dir = temp_dir("imp");
        let codex_home = temp_dir("imp-home");
        let auth_path = codex_home.join("auth.json");
        fs::write(
            &auth_path,
            r#"{
                "OPENAI_API_KEY": null,
                "tokens": {
                    "id_token": "dummy",
                    "access_token": "acc",
                    "refresh_token": "ref",
                    "account_id": "acct-current"
                }
            }"#,
        )
        .unwrap();

        let state = state_with(&dir, Some(&codex_home));

        let imported = match import_codex_local_account(State(state.clone())).await {
            Ok(Json(s)) => s,
            Err(e) => panic!("导入失败：{}", e.message),
        };
        assert_eq!(imported.auth_mode, "chatgpt");
        assert_eq!(imported.account_id.as_deref(), Some("acct-current"));

        let accounts = match list_accounts(State(state)).await {
            Ok(Json(list)) => list,
            Err(e) => panic!("列表失败：{}", e.message),
        };
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].current, "本机在用账号应标记为 current");

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&codex_home).ok();
    }

    #[tokio::test]
    async fn import_codex_local_missing_file_is_not_found() {
        let dir = temp_dir("imp-miss");
        let codex_home = temp_dir("imp-miss-home");
        let state = state_with(&dir, Some(&codex_home));
        match import_codex_local_account(State(state)).await {
            Ok(_) => panic!("缺文件应返回 not found"),
            Err(e) => assert_eq!(e.status, StatusCode::NOT_FOUND),
        }
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&codex_home).ok();
    }

    #[tokio::test]
    async fn import_json_creates_multiple_accounts() {
        let dir = temp_dir("json");
        let state = state_with(&dir, None);
        let req = ImportJsonRequest {
            content: r#"[
                {"type":"codex","access_token":"a1","email":"a@x.com","account_id":"acc-a"},
                {"type":"codex","access_token":"a2","email":"b@x.com","account_id":"acc-b"}
            ]"#
            .to_string(),
        };
        let resp = match import_codex_json_accounts(State(state.clone()), Json(req)).await {
            Ok(Json(r)) => r,
            Err(e) => panic!("导入失败：{}", e.message),
        };
        assert_eq!(resp.imported, 2);

        let accounts = match list_accounts(State(state)).await {
            Ok(Json(list)) => list,
            Err(e) => panic!("列表失败：{}", e.message),
        };
        assert_eq!(accounts.len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn import_json_rejects_garbage() {
        let dir = temp_dir("json-bad");
        let state = state_with(&dir, None);
        let req = ImportJsonRequest {
            content: "{}".to_string(),
        };
        match import_codex_json_accounts(State(state), Json(req)).await {
            Ok(_) => panic!("空对象应被拒绝"),
            Err(e) => assert_eq!(e.status, StatusCode::BAD_REQUEST),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn stats_returns_continuous_series_and_totals() {
        let dir = temp_dir("stats");
        let store = Arc::new(Mutex::new(SessionStore::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.insert_session(&ferry_store::NewSessionRecord {
                provider: "deepseek".into(),
                account: "deepseek-acc1".into(),
                upstream_api: "chat".into(),
                requested_model: "gpt-5".into(),
                target_model: "deepseek-chat".into(),
                stream: false,
                status: ferry_store::SessionStatus::Succeeded,
                duration_ms: 100,
                input_tokens: 3,
                output_tokens: 5,
                total_tokens: 8,
                est_input_tokens: 3,
                est_output_tokens: 5,
                est_total_tokens: 8,
                error: None,
                request_json: serde_json::json!({"model":"gpt-5"}),
                response_json: None,
                output_text: Some("hi".into()),
            })
            .unwrap();
            s.record_account_event("added", "k1", "chatgpt").unwrap();
        }
        let state = IpcState {
            auth_store: Some(AuthStore::with_dir(&dir)),
            session_store: Some(store),
            ..Default::default()
        };

        let resp = match stats(State(state), Query(StatsQuery { days: Some(7) })).await {
            Ok(Json(r)) => r,
            Err(e) => panic!("stats 失败：{}", e.message),
        };
        assert_eq!(resp.series.len(), 7, "应返回连续 7 天");
        assert_eq!(resp.totals.requests, 1);
        assert_eq!(resp.totals.total_tokens, 8);
        assert_eq!(resp.totals.accounts_added, 1);
        assert_eq!(resp.series.last().unwrap().requests, 1, "今天应有 1 次请求");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_pool_accounts_skips_apikey_only() {
        let dir = temp_dir("pool-load");
        let store = AuthStore::with_dir(&dir);
        // OAuth 账号（有 access_token）应纳入；纯 API Key 账号应跳过。
        let oauth = Account {
            provider: "codex".into(),
            id: None,
            email: Some("oauth@x.com".into()),
            account_id: Some("acc-1".into()),
            auth_mode: ferry_auth::AuthMode::Chatgpt,
            tokens: Some(ferry_auth::StoredTokens {
                id_token: "i".into(),
                access_token: "access-tok".into(),
                refresh_token: "r".into(),
            }),
            api_key: None,
            last_refresh: None,
            expires_at: None,
            secret_ref: None,
        };
        let apikey = Account {
            provider: "codex".into(),
            id: None,
            email: Some("key@x.com".into()),
            account_id: None,
            auth_mode: ferry_auth::AuthMode::ApiKey,
            tokens: None,
            api_key: Some("sk-xyz".into()),
            last_refresh: None,
            expires_at: None,
            secret_ref: None,
        };
        store.save(&oauth).unwrap();
        store.save(&apikey).unwrap();

        let accounts = load_pool_accounts(&store);
        assert_eq!(accounts.len(), 1, "仅含 access_token 的账号进池");
        assert_eq!(accounts[0].access_token, "access-tok");
        assert_eq!(accounts[0].base_url, CODEX_BACKEND_BASE_URL);
        assert_eq!(accounts[0].account_id.as_deref(), Some("acc-1"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn host_loopback_detection() {
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("127.0.0.1:15722"));
        assert!(host_is_loopback("localhost:15722"));
        assert!(host_is_loopback("[::1]:15722"));
        assert!(host_is_loopback("::1"));
        assert!(!host_is_loopback("evil.com"));
        assert!(!host_is_loopback("192.168.1.10:15722"));
    }

    #[test]
    fn normalize_weather_extracts_summary() {
        let data = serde_json::json!({
            "location": {"city": "北京"},
            "forecast_keypoint": "未来两小时不会下雨",
            "summary": {
                "skycon": "阴", "skycon_emoji": "☁️", "skycon_code": "CLOUDY",
                "temperature": 27.8, "apparent_temperature": 29.5,
                "humidity_percent": 58, "visibility_km": 32.6,
                "wind": {"direction_text": "东南偏南风", "level_text": "微风"},
                "air_quality": {"aqi": 50, "level": "优", "level_color": "green", "pm25": 3}
            },
            "alerts": [{"title": "大风蓝色预警", "color": "蓝色", "level": "一般"}]
        });
        let w = normalize_weather(&data, "上海");
        assert_eq!(w.city, "北京");
        assert_eq!(w.skycon, "阴");
        assert_eq!(w.emoji, "☁️");
        assert_eq!(w.temperature, Some(27.8));
        assert_eq!(w.wind_text, "东南偏南风");
        assert_eq!(w.aqi_level, "优");
        assert_eq!(w.aqi_color, "green");
        assert_eq!(w.alerts.len(), 1);
        assert_eq!(w.alerts[0].color, "蓝色");
        assert_eq!(w.forecast_keypoint.as_deref(), Some("未来两小时不会下雨"));
    }

    #[test]
    fn normalize_weather_uses_fallback_city_when_missing() {
        let data = serde_json::json!({ "summary": {} });
        let w = normalize_weather(&data, "深圳");
        assert_eq!(w.city, "深圳");
    }

    #[test]
    fn parse_usage_body_reads_nested_windows() {
        let json = serde_json::json!({
            "plan_type": "plus",
            "primary": {"used_percent": 39.0, "limit_window_seconds": 18000, "reset_at": 1900000000_i64},
            "secondary": {"used_percent": 15.0, "window_minutes": 10080, "resets_in_seconds": 3600}
        });
        let q = parse_usage_body(&json).unwrap();
        assert_eq!(q.plan_type.as_deref(), Some("plus"));
        assert_eq!(q.primary.used_percent, Some(39.0));
        assert_eq!(q.primary.window_minutes, Some(300));
        assert_eq!(q.primary.reset_at, Some(1900000000));
        assert_eq!(q.secondary.used_percent, Some(15.0));
        assert!(q.secondary.reset_at.unwrap() > Utc::now().timestamp());
    }

    #[test]
    fn parse_usage_body_reads_cockpit_rate_limit() {
        // 对照 cockpit-tools 的 wham/usage 响应：5h/7d 在 rate_limit.{primary,secondary}_window，
        // used_percent 为整数、窗口用 limit_window_seconds、重置用 reset_after_seconds。
        let json = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 42, "limit_window_seconds": 18000, "reset_after_seconds": 600},
                "secondary_window": {"used_percent": 8, "limit_window_seconds": 604800, "reset_after_seconds": 86400}
            }
        });
        let q = parse_usage_body(&json).unwrap();
        assert_eq!(q.plan_type.as_deref(), Some("pro"));
        assert_eq!(q.primary.used_percent, Some(42.0));
        assert_eq!(q.primary.window_minutes, Some(300));
        assert!(q.primary.reset_at.unwrap() > Utc::now().timestamp());
        assert_eq!(q.secondary.used_percent, Some(8.0));
        assert_eq!(q.secondary.window_minutes, Some(10080));
    }

    #[test]
    fn parse_usage_body_empty_returns_none() {
        assert!(parse_usage_body(&serde_json::json!({})).is_none());
        assert!(parse_usage_body(&serde_json::Value::Null).is_none());
    }

    #[tokio::test]
    async fn settings_save_and_get_roundtrip() {
        let dir = temp_dir("settings");
        let state = IpcState {
            auth_store: Some(AuthStore::with_dir(&dir)),
            settings_store: Some(ferry_config::SettingsStore::with_path(
                dir.join("settings.json"),
            )),
            ..Default::default()
        };
        // 默认值：城市留空（自动定位）、自动定位默认开。
        let Json(resp0) = get_settings(State(state.clone())).await;
        assert_eq!(resp0.settings.weather_city, "");
        assert!(resp0.settings.weather_auto_locate, "默认应自动定位");
        assert!(!resp0.apizero_key_configured);

        // 保存自定义设置。
        let custom = AppSettings {
            weather_city: "杭州".into(),
            weather_auto_locate: false,
            show_weather: false,
            show_poem: true,
            poem_category: "shanshui".into(),
            pool_quota_aware: false,
        };
        let saved = save_settings(State(state.clone()), Json(custom.clone())).await;
        assert!(saved.is_ok());
        let Json(resp1) = get_settings(State(state.clone())).await;
        assert_eq!(resp1.settings.weather_city, "杭州");
        assert!(!resp1.settings.show_weather);
        assert_eq!(resp1.settings.poem_category, "shanshui");

        // apizero key 配置与回显。
        let ok = set_apizero_key(
            State(state.clone()),
            Json(ApizeroKeyRequest {
                api_key: "az-123".into(),
            }),
        )
        .await;
        assert!(ok.is_ok());
        let Json(resp2) = get_settings(State(state.clone())).await;
        assert!(resp2.apizero_key_configured);

        // 清除 key。
        let _ = set_apizero_key(
            State(state.clone()),
            Json(ApizeroKeyRequest {
                api_key: "  ".into(),
            }),
        )
        .await;
        let Json(resp3) = get_settings(State(state)).await;
        assert!(!resp3.apizero_key_configured);
        fs::remove_dir_all(&dir).ok();
    }
}
