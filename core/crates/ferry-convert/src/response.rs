//! 非流式响应转换：上游 Chat Completions 完整响应 -> Responses 响应。

use serde_json::{json, Value};

use crate::{convert_usage, gen_id};

/// 将上游 Chat Completions 的完整（非流式）响应转换为 Responses 响应。
pub fn chat_response_to_responses(chat: &Value, model: &str) -> Value {
    let message = chat
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"));

    let content = message
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut output = Vec::new();

    // tool_calls -> function_call 条目
    if let Some(tool_calls) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(Value::as_array)
    {
        for tc in tool_calls {
            let func = tc.get("function");
            output.push(json!({
                "type": "function_call",
                "id": gen_id("fc"),
                "call_id": tc.get("id").cloned().unwrap_or(json!("")),
                "name": func.and_then(|f| f.get("name")).cloned().unwrap_or(json!("")),
                "arguments": func.and_then(|f| f.get("arguments")).cloned().unwrap_or(json!("{}")),
                "status": "completed"
            }));
        }
    }

    // 有文本内容，或没有任何工具调用时，输出一个 message 条目
    if !content.is_empty() || output.is_empty() {
        output.push(json!({
            "id": gen_id("msg"),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type":"output_text","text": content, "annotations": []}]
        }));
    }

    let mut resp = json!({
        "id": gen_id("resp"),
        "object": "response",
        "status": "completed",
        "model": model,
        "output": output,
        "output_text": content,
    });
    if let Some(u) = chat.get("usage") {
        resp["usage"] = convert_usage(u);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_text_response() {
        let chat = json!({
            "choices":[{"message":{"role":"assistant","content":"Hi there"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}
        });
        let r = chat_response_to_responses(&chat, "deepseek-chat");
        assert_eq!(r["status"], "completed");
        assert_eq!(r["output_text"], "Hi there");
        assert_eq!(r["output"][0]["content"][0]["text"], "Hi there");
        assert_eq!(r["usage"]["output_tokens"], 2);
        assert_eq!(r["usage"]["total_tokens"], 3);
    }

    #[test]
    fn tool_call_response() {
        let chat = json!({
            "choices":[{"message":{
                "role":"assistant","content":null,
                "tool_calls":[{"id":"c1","type":"function","function":{"name":"f","arguments":"{}"}}]
            },"finish_reason":"tool_calls"}]
        });
        let r = chat_response_to_responses(&chat, "m");
        assert_eq!(r["output"][0]["type"], "function_call");
        assert_eq!(r["output"][0]["name"], "f");
        assert_eq!(r["output"][0]["call_id"], "c1");
    }
}
