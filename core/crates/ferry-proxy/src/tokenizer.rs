//! 本地 token 估算：用 tiktoken（o200k_base）独立数 token，与上游 `usage` 上报对比，
//! 用于识别「中转掺假 / 虚报 token」。对国产模型分词不完全一致，故为**估算**，
//! 适合做数量级对比（如上游报数是本地估算的数倍即疑似掺假），不做精确计费。

use std::sync::OnceLock;

use serde_json::Value;
use tiktoken_rs::{o200k_base, CoreBPE};

/// 进程内复用一份 BPE（加载有成本）。
fn bpe() -> Option<&'static CoreBPE> {
    static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| match o200k_base() {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!("加载 o200k_base 分词器失败，token 估算停用: {e}");
            None
        }
    })
    .as_ref()
}

/// 估算一段文本的 token 数；分词器不可用时返回 0。
pub fn count_text_tokens(text: &str) -> i64 {
    if text.is_empty() {
        return 0;
    }
    match bpe() {
        Some(b) => b.encode_with_special_tokens(text).len() as i64,
        None => 0,
    }
}

/// 从 Chat 请求体的 `messages` 估算输入 token（含每条消息固定开销近似）。
pub fn count_chat_input_tokens(chat_body: &Value) -> i64 {
    let Some(messages) = chat_body.get("messages").and_then(Value::as_array) else {
        return 0;
    };
    let mut total = 0i64;
    for msg in messages {
        // 每条消息的角色/分隔符固定开销，对齐 OpenAI 计数惯例（约 4 token/条）。
        total += 4;
        if let Some(content) = msg.get("content") {
            total += count_content_tokens(content);
        }
        if let Some(name) = msg.get("name").and_then(Value::as_str) {
            total += count_text_tokens(name);
        }
    }
    total + 2 // 回复引导开销
}

/// content 可能是字符串或多模态片段数组（取其中的 text）。
fn count_content_tokens(content: &Value) -> i64 {
    match content {
        Value::String(s) => count_text_tokens(s),
        Value::Array(parts) => parts
            .iter()
            .map(|p| {
                p.get("text")
                    .and_then(Value::as_str)
                    .map(count_text_tokens)
                    .unwrap_or(0)
            })
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_nonempty_text() {
        assert_eq!(count_text_tokens(""), 0);
        assert!(count_text_tokens("hello world, this is a test") >= 5);
    }

    #[test]
    fn counts_chat_messages_input() {
        let body = serde_json::json!({
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello!"}
            ]
        });
        // 两条消息固定开销 8 + 文本 + 引导 2，至少大于纯文本数量。
        assert!(count_chat_input_tokens(&body) > count_text_tokens("Hello!"));
    }

    #[test]
    fn handles_multimodal_content_array() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "describe this"},
                    {"type": "image_url", "image_url": {"url": "data:..."}}
                ]}
            ]
        });
        assert!(count_chat_input_tokens(&body) > 0);
    }
}
