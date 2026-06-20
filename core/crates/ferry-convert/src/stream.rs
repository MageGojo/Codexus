//! 流式响应转换：上游 Chat SSE chunk 序列 -> Codex 可理解的 Responses 事件序列。

use serde_json::{json, Value};

use crate::{convert_usage, gen_id};

/// 流式转换器。逐个喂入上游 Chat 的 SSE chunk（已解析为 JSON），
/// 产出若干完整的 SSE 帧（`event: <type>\ndata: <json>\n\n`）转发给 Codex。
pub struct StreamConverter {
    response_id: String,
    item_id: String,
    model: String,
    created_sent: bool,
    item_open: bool,
    accumulated: String,
    seq: u64,
    finish_reason: Option<String>,
}

impl StreamConverter {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            response_id: gen_id("resp"),
            item_id: gen_id("msg"),
            model: model.into(),
            created_sent: false,
            item_open: false,
            accumulated: String::new(),
            seq: 0,
            finish_reason: None,
        }
    }

    fn frame(&mut self, typ: &str, mut data: Value) -> String {
        data["type"] = json!(typ);
        data["sequence_number"] = json!(self.seq);
        self.seq += 1;
        format!(
            "event: {typ}\ndata: {}\n\n",
            serde_json::to_string(&data).unwrap_or_else(|_| "{}".into())
        )
    }

    fn response_object(&self, status: &str, usage: Option<&Value>) -> Value {
        let output = if status == "completed" && self.item_open {
            json!([self.message_item("completed")])
        } else {
            json!([])
        };
        let mut r = json!({
            "id": self.response_id,
            "object": "response",
            "status": status,
            "model": self.model,
            "output": output,
        });
        if let Some(u) = usage {
            r["usage"] = convert_usage(u);
        }
        r
    }

    fn message_item(&self, status: &str) -> Value {
        json!({
            "id": self.item_id,
            "type": "message",
            "status": status,
            "role": "assistant",
            "content": [{"type":"output_text","text": self.accumulated, "annotations": []}]
        })
    }

    fn ensure_started(&mut self, out: &mut Vec<String>) {
        if self.created_sent {
            return;
        }
        let created = self.response_object("in_progress", None);
        out.push(self.frame("response.created", json!({ "response": created })));
        let inprog = self.response_object("in_progress", None);
        out.push(self.frame("response.in_progress", json!({ "response": inprog })));
        self.created_sent = true;
    }

    fn ensure_item(&mut self, out: &mut Vec<String>) {
        if self.item_open {
            return;
        }
        let item_id = self.item_id.clone();
        let item = json!({
            "id": item_id, "type": "message", "status": "in_progress",
            "role": "assistant", "content": []
        });
        out.push(self.frame(
            "response.output_item.added",
            json!({
                "output_index": 0, "item": item
            }),
        ));
        let item_id = self.item_id.clone();
        out.push(self.frame(
            "response.content_part.added",
            json!({
                "item_id": item_id, "output_index": 0, "content_index": 0,
                "part": {"type":"output_text","text":"","annotations":[]}
            }),
        ));
        self.item_open = true;
    }

    /// 处理一个上游 chat chunk，产出对应的 Responses SSE 帧。
    pub fn push_chat_chunk(&mut self, chunk: &Value) -> Vec<String> {
        let mut out = Vec::new();
        self.ensure_started(&mut out);

        if let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) {
            if let Some(text) = choice
                .get("delta")
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if !text.is_empty() {
                    self.ensure_item(&mut out);
                    self.accumulated.push_str(text);
                    let item_id = self.item_id.clone();
                    let delta = text.to_string();
                    out.push(self.frame(
                        "response.output_text.delta",
                        json!({
                            "item_id": item_id, "output_index": 0,
                            "content_index": 0, "delta": delta
                        }),
                    ));
                }
            }
            if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(fr.to_string());
            }
        }
        out
    }

    /// 结束流：补齐 done 系列事件，并发出 `response.completed`。
    pub fn finish(&mut self, usage: Option<&Value>) -> Vec<String> {
        let mut out = Vec::new();
        self.ensure_started(&mut out);

        if self.item_open {
            let item_id = self.item_id.clone();
            let text = self.accumulated.clone();
            out.push(self.frame(
                "response.output_text.done",
                json!({
                    "item_id": item_id, "output_index": 0,
                    "content_index": 0, "text": text
                }),
            ));
            let item_id = self.item_id.clone();
            let acc = self.accumulated.clone();
            out.push(self.frame(
                "response.content_part.done",
                json!({
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "part": {"type":"output_text","text": acc, "annotations": []}
                }),
            ));
            let item = self.message_item("completed");
            out.push(self.frame(
                "response.output_item.done",
                json!({
                    "output_index": 0, "item": item
                }),
            ));
        }
        let completed = self.response_object("completed", usage);
        out.push(self.frame("response.completed", json!({ "response": completed })));
        out
    }

    /// 已累计的完整文本（用于会话落库等）。
    pub fn accumulated_text(&self) -> &str {
        &self.accumulated
    }

    /// 上游给出的 finish_reason（若有）。
    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// 当前响应使用的模型名。
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_types(frames: &[String]) -> Vec<String> {
        frames
            .iter()
            .filter_map(|f| {
                f.lines()
                    .find_map(|l| l.strip_prefix("event: ").map(str::to_string))
            })
            .collect()
    }

    #[test]
    fn basic_stream_flow() {
        let mut c = StreamConverter::new("deepseek-chat");
        let mut frames = Vec::new();
        frames.extend(c.push_chat_chunk(&json!({
            "choices":[{"delta":{"content":"Hel"},"finish_reason":null}]
        })));
        frames.extend(c.push_chat_chunk(&json!({
            "choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]
        })));
        frames.extend(c.finish(Some(&json!({
            "prompt_tokens":3,"completion_tokens":2,"total_tokens":5
        }))));

        let types = event_types(&frames);
        assert!(types.contains(&"response.created".to_string()));
        assert!(types.contains(&"response.output_item.added".to_string()));
        assert!(types.contains(&"response.output_text.delta".to_string()));
        assert!(types.contains(&"response.completed".to_string()));

        assert_eq!(c.accumulated_text(), "Hello");
        assert_eq!(c.finish_reason(), Some("stop"));

        let completed = frames
            .iter()
            .find(|f| f.contains("response.completed"))
            .unwrap();
        assert!(completed.contains("\"input_tokens\":3"));
        assert!(completed.contains("Hello"));
    }

    #[test]
    fn empty_stream_still_completes() {
        let mut c = StreamConverter::new("m");
        let frames = c.finish(None);
        let types = event_types(&frames);
        assert!(types.contains(&"response.created".to_string()));
        assert!(types.contains(&"response.completed".to_string()));
    }
}
