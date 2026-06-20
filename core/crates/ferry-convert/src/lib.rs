//! ferry-convert：Codex Responses API <-> OpenAI Chat Completions 协议转换。
//!
//! Codex 以 `wire_api = "responses"` 发送请求；而国内多数模型（DeepSeek / 通义 /
//! Kimi / GLM 等）仅提供 Chat Completions 兼容接口。本 crate 负责双向转换：
//!
//! - 请求：Responses 请求体 -> Chat 请求体（见 [`responses_to_chat_request`]）
//! - 响应（流式）：上游 Chat SSE chunk -> Responses 事件（见 [`StreamConverter`]）
//! - 响应（非流式）：Chat 响应 -> Responses 响应（见 [`chat_response_to_responses`]）

mod request;
mod response;
mod stream;

pub use request::responses_to_chat_request;
pub use response::chat_response_to_responses;
pub use stream::StreamConverter;

use serde_json::{json, Value};

/// 上游接口协议类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamApi {
    /// OpenAI Chat Completions 兼容（DeepSeek / 通义 / Kimi / GLM 等）。
    Chat,
    /// OpenAI Responses 兼容（官方 / 部分中转），可基本透传。
    Responses,
}

/// 生成带前缀的唯一 id（如 `resp_xxx` / `msg_xxx`）。
pub(crate) fn gen_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// 从 Responses 的 content 字段（字符串或 part 数组）提取纯文本。
pub(crate) fn extract_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// 把任意 JSON 值转为文本（function_call_output 的 output 可能是字符串/数组/对象）。
pub(crate) fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(_) => extract_text(Some(v)),
        other => other.to_string(),
    }
}

/// Chat usage -> Responses usage 字段命名转换。
pub(crate) fn convert_usage(u: &Value) -> Value {
    json!({
        "input_tokens": u.get("prompt_tokens").cloned().unwrap_or(json!(0)),
        "output_tokens": u.get("completion_tokens").cloned().unwrap_or(json!(0)),
        "total_tokens": u.get("total_tokens").cloned().unwrap_or(json!(0)),
    })
}
