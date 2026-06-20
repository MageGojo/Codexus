//! 请求转换：Codex Responses 请求体 -> OpenAI Chat Completions 请求体。

use serde_json::{json, Value};

use crate::{extract_text, value_to_text};

/// 将 Codex 的 Responses 请求体转换为 Chat Completions 请求体。
///
/// `target_model` 为映射后的真实上游模型名（如 `deepseek-chat`）。
pub fn responses_to_chat_request(req: &Value, target_model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    // instructions -> system 消息（置于最前）
    if let Some(instr) = req.get("instructions").and_then(Value::as_str) {
        if !instr.is_empty() {
            messages.push(json!({ "role": "system", "content": instr }));
        }
    }

    // input 可能是字符串或条目数组
    match req.get("input") {
        Some(Value::String(s)) => messages.push(json!({ "role": "user", "content": s })),
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(m) = convert_input_item(item) {
                    messages.push(m);
                }
            }
        }
        _ => {}
    }

    let mut chat = json!({
        "model": target_model,
        "messages": messages,
    });

    // 透传 / 改名常见采样与控制参数
    if let Some(v) = req.get("stream") {
        chat["stream"] = v.clone();
    }
    if let Some(v) = req.get("temperature") {
        chat["temperature"] = v.clone();
    }
    if let Some(v) = req.get("top_p") {
        chat["top_p"] = v.clone();
    }
    if let Some(v) = req.get("max_output_tokens") {
        chat["max_tokens"] = v.clone();
    }
    if let Some(v) = req.get("tools") {
        if let Some(tools) = convert_tools(v) {
            chat["tools"] = tools;
        }
    }
    if let Some(v) = req.get("tool_choice") {
        chat["tool_choice"] = v.clone();
    }
    if let Some(v) = req.get("parallel_tool_calls") {
        chat["parallel_tool_calls"] = v.clone();
    }

    chat
}

/// 把 Responses 的消息角色映射为 Chat Completions 兼容角色。
///
/// 关键：Codex / 新版 OpenAI 用 `developer` 表示系统级指令，但绝大多数
/// Chat Completions 上游（DeepSeek / Qwen / Kimi / GLM 及多数中转）只认
/// `system / user / assistant / tool`——必须把 `developer` 映射为 `system`，
/// 否则上游报 `unknown variant 'developer'`。未知角色保守降级为 `user`，
/// 避免再触发反序列化失败。
fn map_chat_role(role: &str) -> &str {
    match role {
        "developer" => "system",
        "system" | "user" | "assistant" | "tool" => role,
        _ => "user",
    }
}

/// 转换单个 Responses input 条目为 Chat message。
fn convert_input_item(item: &Value) -> Option<Value> {
    let typ = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    match typ {
        "message" => {
            let raw_role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let role = map_chat_role(raw_role);
            Some(json!({ "role": role, "content": extract_text(item.get("content")) }))
        }
        "function_call" => {
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                }]
            }))
        }
        "function_call_output" => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let output = item.get("output").map(value_to_text).unwrap_or_default();
            Some(json!({ "role": "tool", "tool_call_id": call_id, "content": output }))
        }
        // reasoning 等其它类型暂忽略（后续里程碑迭代）
        _ => None,
    }
}

/// Responses tools -> Chat tools。
///
/// Responses function tool 形如 `{type:"function", name, description, parameters}`，
/// Chat 需要 `{type:"function", function:{name, description, parameters}}`。
fn convert_tools(tools: &Value) -> Option<Value> {
    let arr = tools.as_array()?;
    let mut out = Vec::new();
    for t in arr {
        let typ = t.get("type").and_then(Value::as_str).unwrap_or("function");
        if typ != "function" {
            continue; // web_search 等暂不支持
        }
        if t.get("function").is_some() {
            out.push(t.clone()); // 已是 chat 形态
        } else {
            out.push(json!({
                "type": "function",
                "function": {
                    "name": t.get("name").cloned().unwrap_or(json!("")),
                    "description": t.get("description").cloned().unwrap_or(json!("")),
                    "parameters": t.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
                }
            }));
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Array(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_and_string_input() {
        let req = json!({
            "model": "gpt-5",
            "instructions": "You are helpful",
            "input": "hello"
        });
        let chat = responses_to_chat_request(&req, "deepseek-chat");
        assert_eq!(chat["model"], "deepseek-chat");
        let msgs = chat["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hello");
    }

    #[test]
    fn array_input_with_parts() {
        let req = json!({
            "input": [
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"foo "},
                    {"type":"input_text","text":"bar"}
                ]}
            ]
        });
        let chat = responses_to_chat_request(&req, "qwen-plus");
        let msgs = chat["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"], "foo bar");
    }

    #[test]
    fn tool_call_and_output() {
        let req = json!({
            "input": [
                {"type":"function_call","name":"get_weather","arguments":"{\"city\":\"bj\"}","call_id":"c1"},
                {"type":"function_call_output","call_id":"c1","output":"sunny"}
            ]
        });
        let chat = responses_to_chat_request(&req, "m");
        let msgs = chat["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "c1");
        assert_eq!(msgs[1]["content"], "sunny");
    }

    #[test]
    fn developer_role_mapped_to_system() {
        // Codex 常把系统指令作为 developer 消息发来；上游（DeepSeek 等）只认 system。
        let req = json!({
            "input": [
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"sys rules"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}
            ]
        });
        let chat = responses_to_chat_request(&req, "deepseek-chat");
        let msgs = chat["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system", "developer 应映射为 system");
        assert_eq!(msgs[0]["content"], "sys rules");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn unknown_role_falls_back_to_user() {
        let req = json!({
            "input": [
                {"type":"message","role":"weird","content":[{"type":"input_text","text":"x"}]}
            ]
        });
        let chat = responses_to_chat_request(&req, "m");
        assert_eq!(chat["messages"][0]["role"], "user", "未知角色应保守降级为 user");
    }

    #[test]
    fn standard_roles_pass_through() {
        for r in ["system", "user", "assistant", "tool"] {
            let req = json!({
                "input": [
                    {"type":"message","role":r,"content":[{"type":"input_text","text":"x"}]}
                ]
            });
            let chat = responses_to_chat_request(&req, "m");
            assert_eq!(chat["messages"][0]["role"], r, "标准角色应原样透传");
        }
    }

    #[test]
    fn tools_conversion() {
        let req = json!({
            "input": "hi",
            "tools": [{"type":"function","name":"f","description":"d","parameters":{"type":"object"}}]
        });
        let chat = responses_to_chat_request(&req, "m");
        assert_eq!(chat["tools"][0]["function"]["name"], "f");
        assert_eq!(chat["tools"][0]["type"], "function");
    }
}
