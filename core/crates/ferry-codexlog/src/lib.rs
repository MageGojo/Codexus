//! ferry-codexlog：解析 Codex CLI 自己写的会话文件（rollout），统计 token 用量与会话列表。
//!
//! Codex CLI 把每个会话写为 `$CODEX_HOME/sessions/<年>/<月>/<日>/rollout-<id>.jsonl`
//! （以及 `archived_sessions/`）。每行一个 JSON：
//! - 首行 `session_meta`：会话 id / cwd / 时间戳。
//! - token 行 `{"type":"event_msg","payload":{"type":"token_count",
//!   "info":{"total_token_usage":{"input_tokens","output_tokens","total_tokens"}}}}`，
//!   末尾一条是该会话的**累计**用量。
//!
//! 该模块对「OAuth 直连官方」与「供应商经 ferry-proxy」两类账号都适用：只要 Codex CLI
//! 在跑，它就会写 rollout，故 token 统计来源统一、且为上游返回的真实 usage。

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// 会话目录（活跃 + 归档）。
const SESSION_DIRS: [&str; 2] = ["sessions", "archived_sessions"];
/// 读取文件尾部用于查找最后一条 token 用量的最大字节数。
const TAIL_READ_BYTES: u64 = 512 * 1024;
/// 读取文件头部用于解析会话元信息 / 标题的最大字节数。
const HEAD_READ_BYTES: usize = 128 * 1024;
/// 会话标题最大字符数。
const TITLE_MAX_CHARS: usize = 80;

/// 单个会话记录（含该会话累计 token 用量）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    /// 文件修改时间（Unix 秒）。
    pub updated_at: Option<i64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// 跨会话聚合用量。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub session_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// 会话中的一条消息（用户 / 助手）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
}

/// 单个会话的完整详情（含聊天记录）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub updated_at: Option<i64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub messages: Vec<ChatMessage>,
}

/// Codex 会话日志读取器（绑定一个 `$CODEX_HOME`）。
pub struct CodexLog {
    home: PathBuf,
}

impl CodexLog {
    /// 显式指定 Codex home 目录。
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// 解析默认 Codex home：优先 `CODEX_HOME`，否则 `~/.codex`。
    pub fn locate() -> Self {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| home_dir().join(".codex"));
        Self { home }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// 收集所有 rollout 文件路径（活跃 + 归档）。
    fn rollout_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for dir in SESSION_DIRS {
            collect_rollout_files(&self.home.join(dir), &mut files);
        }
        files
    }

    /// 列出所有会话，按更新时间倒序（最新在前），同一 session_id 仅保留最新一条。
    pub fn list_sessions(&self) -> Vec<SessionRecord> {
        let mut out: Vec<SessionRecord> = self
            .rollout_files()
            .iter()
            .filter_map(|path| read_session_record(path))
            .collect();
        out.sort_by(|a, b| {
            b.updated_at
                .unwrap_or_default()
                .cmp(&a.updated_at.unwrap_or_default())
                .then_with(|| b.total_tokens.cmp(&a.total_tokens))
        });
        dedup_by_id(out)
    }

    /// 读取单个会话的完整详情（含聊天记录），按 session_id 查找对应 rollout。
    pub fn read_session_detail(&self, session_id: &str) -> Option<SessionDetail> {
        let path = self.find_rollout_path(session_id)?;
        let (sid, cwd, title) = read_head(&path)?;
        let (input_tokens, output_tokens, total_tokens) =
            read_last_token_usage(&path).unwrap_or((0, 0, 0));
        let updated_at = fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        Some(SessionDetail {
            session_id: sid,
            title,
            cwd,
            updated_at,
            input_tokens,
            output_tokens,
            total_tokens,
            messages: read_conversation(&path),
        })
    }

    fn find_rollout_path(&self, session_id: &str) -> Option<PathBuf> {
        if session_id.trim().is_empty() {
            return None;
        }
        for path in self.rollout_files() {
            let name_hit = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|stem| stem.contains(session_id))
                .unwrap_or(false);
            if name_hit {
                return Some(path);
            }
            if let Some((id, _, _)) = read_head(&path) {
                if id == session_id {
                    return Some(path);
                }
            }
        }
        None
    }

    /// 跨会话聚合 token 用量。
    pub fn usage_summary(&self) -> UsageSummary {
        let sessions = self.list_sessions();
        let mut summary = UsageSummary {
            session_count: sessions.len(),
            ..Default::default()
        };
        for record in &sessions {
            summary.input_tokens = summary.input_tokens.saturating_add(record.input_tokens);
            summary.output_tokens = summary.output_tokens.saturating_add(record.output_tokens);
            summary.total_tokens = summary.total_tokens.saturating_add(record.total_tokens);
        }
        summary
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn collect_rollout_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => collect_rollout_files(&path, out),
            Ok(ft) if ft.is_file() && is_rollout_file(&path) => out.push(path),
            _ => {}
        }
    }
}

fn is_rollout_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        .unwrap_or(false)
}

fn read_session_record(path: &Path) -> Option<SessionRecord> {
    let (session_id, cwd, title) = read_head(path)?;
    let (input_tokens, output_tokens, total_tokens) =
        read_last_token_usage(path).unwrap_or((0, 0, 0));
    let updated_at = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    Some(SessionRecord {
        session_id,
        title,
        cwd,
        updated_at,
        input_tokens,
        output_tokens,
        total_tokens,
    })
}

/// 读取文件头部，解析 (session_id, cwd, title)。
fn read_head(path: &Path) -> Option<(String, String, String)> {
    let mut file = File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD_READ_BYTES];
    let n = file.read(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);

    let mut session_id = String::new();
    let mut cwd = String::new();
    let mut title = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if session_id.is_empty() {
            if let Some(id) = extract_session_id(&value) {
                session_id = id;
                cwd = extract_cwd(&value).unwrap_or_default();
            }
        }
        if title.is_empty() {
            if let Some(t) = extract_user_title(&value) {
                title = t;
            }
        }
        if !session_id.is_empty() && !title.is_empty() {
            break;
        }
    }

    if session_id.is_empty() {
        session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
    }
    if session_id.is_empty() {
        return None;
    }
    if title.is_empty() {
        title = if cwd.is_empty() {
            session_id.clone()
        } else {
            Path::new(&cwd)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(cwd.as_str())
                .to_string()
        };
    }
    Some((session_id, cwd, title))
}

fn extract_session_id(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) == Some("session_meta") {
        if let Some(id) = value.pointer("/payload/id").and_then(Value::as_str) {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    // 老格式：顶层 id 且带 meta 特征字段。
    let has_meta_markers = value.get("cwd").is_some()
        || value.get("instructions").is_some()
        || value.get("git").is_some();
    if has_meta_markers {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn extract_cwd(value: &Value) -> Option<String> {
    value
        .pointer("/payload/cwd")
        .and_then(Value::as_str)
        .or_else(|| value.get("cwd").and_then(Value::as_str))
        .map(|s| s.to_string())
}

/// 尝试从一行里提取首条用户消息文本作为标题。
fn extract_user_title(value: &Value) -> Option<String> {
    let payload = value.get("payload").unwrap_or(value);

    if payload.get("role").and_then(Value::as_str) == Some("user") {
        if let Some(text) = extract_text_from_content(payload.get("content")) {
            return clean_title(&text);
        }
    }
    if payload.get("type").and_then(Value::as_str) == Some("user_message") {
        if let Some(message) = payload.get("message").and_then(Value::as_str) {
            return clean_title(message);
        }
    }
    None
}

fn extract_text_from_content(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(items) = content.as_array() {
        let mut buf = String::new();
        for item in items {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                buf.push_str(text);
            }
        }
        if !buf.is_empty() {
            return Some(buf);
        }
    }
    None
}

fn clean_title(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 跳过 Codex 注入的环境 / 指令块（如 <environment_context>...、<user_instructions>...）。
    if trimmed.starts_with('<') {
        return None;
    }
    let one_line = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return None;
    }
    Some(one_line.chars().take(TITLE_MAX_CHARS).collect())
}

/// 解析 rollout 全文，提取 user/assistant 消息文本作为聊天记录。
fn read_conversation(path: &Path) -> Vec<ChatMessage> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        let Some(role) = payload.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let Some(text) = extract_text_from_content(payload.get("content")) else {
            continue;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        // 跳过 Codex 注入的环境 / 指令块（user 首条常是 <environment_context> 等）。
        if role == "user" && text.starts_with('<') {
            continue;
        }
        out.push(ChatMessage {
            role: role.to_string(),
            text,
        });
    }
    out
}

/// 从文件尾部往前找最后一条 `total_token_usage`，返回 (input, output, total)。
fn read_last_token_usage(path: &Path) -> Option<(u64, u64, u64)> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let start = len.saturating_sub(TAIL_READ_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);

    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty()
            || !line.contains("token_count")
            || !line.contains("total_token_usage")
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(usage) = find_total_token_usage(&value) {
            let input = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let total = usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
            return Some((input, output, total));
        }
    }
    None
}

fn find_total_token_usage(value: &Value) -> Option<&Value> {
    value
        .pointer("/payload/info/total_token_usage")
        .or_else(|| value.pointer("/info/total_token_usage"))
        .or_else(|| value.pointer("/payload/total_token_usage"))
}

fn dedup_by_id(records: Vec<SessionRecord>) -> Vec<SessionRecord> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(records.len());
    for record in records {
        if seen.insert(record.session_id.clone()) {
            out.push(record);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn temp_home() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ferry-codexlog-test-{}-{}-{}",
            std::process::id(),
            nanos(),
            unique
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_rollout(home: &Path, rel: &str, lines: &[&str]) {
        let path = home.join("sessions").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    #[test]
    fn reads_token_usage_meta_and_title() {
        let home = temp_home();
        write_rollout(
            &home,
            "2026/06/19/rollout-2026-06-19T10-00-00-abc123.jsonl",
            &[
                r#"{"timestamp":"2026-06-19T10:00:00Z","type":"session_meta","payload":{"id":"abc123","cwd":"/Users/demo/proj","timestamp":"2026-06-19T10:00:00Z"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"帮我写个快速排序"}]}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":120,"total_tokens":420}}}}"#,
            ],
        );

        let log = CodexLog::with_home(&home);
        let sessions = log.list_sessions();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.session_id, "abc123");
        assert_eq!(s.cwd, "/Users/demo/proj");
        assert_eq!(s.title, "帮我写个快速排序");
        // 取最后一条累计用量
        assert_eq!(s.input_tokens, 300);
        assert_eq!(s.output_tokens, 120);
        assert_eq!(s.total_tokens, 420);

        let summary = log.usage_summary();
        assert_eq!(summary.session_count, 1);
        assert_eq!(summary.total_tokens, 420);

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn reads_session_detail_with_conversation() {
        let home = temp_home();
        write_rollout(
            &home,
            "2026/06/19/rollout-2026-06-19T11-00-00-conv1.jsonl",
            &[
                r#"{"type":"session_meta","payload":{"id":"conv1","cwd":"/Users/demo/app"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd=/x</environment_context>"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"你好"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"你好，有什么可以帮你"}]}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":12,"output_tokens":8,"total_tokens":20}}}}"#,
            ],
        );

        let detail = CodexLog::with_home(&home)
            .read_session_detail("conv1")
            .expect("detail");
        assert_eq!(detail.session_id, "conv1");
        assert_eq!(detail.total_tokens, 20);
        // 环境块被跳过，仅保留真实对话两条。
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].role, "user");
        assert_eq!(detail.messages[0].text, "你好");
        assert_eq!(detail.messages[1].role, "assistant");
        assert_eq!(detail.messages[1].text, "你好，有什么可以帮你");

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn aggregates_multiple_sessions_and_dedups() {
        let home = temp_home();
        write_rollout(
            &home,
            "2026/06/18/rollout-2026-06-18T09-00-00-aaa.jsonl",
            &[
                r#"{"type":"session_meta","payload":{"id":"aaa","cwd":"/x"}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}"#,
            ],
        );
        write_rollout(
            &home,
            "2026/06/19/rollout-2026-06-19T09-00-00-bbb.jsonl",
            &[
                r#"{"type":"session_meta","payload":{"id":"bbb","cwd":"/y"}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"output_tokens":7,"total_tokens":27}}}}"#,
            ],
        );

        let summary = CodexLog::with_home(&home).usage_summary();
        assert_eq!(summary.session_count, 2);
        assert_eq!(summary.input_tokens, 30);
        assert_eq!(summary.total_tokens, 42);

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_home_yields_empty() {
        let home = temp_home();
        let _ = fs::remove_dir_all(&home);
        let log = CodexLog::with_home(&home);
        assert_eq!(log.list_sessions().len(), 0);
        assert_eq!(log.usage_summary().total_tokens, 0);
    }

    #[test]
    fn ignores_non_rollout_files() {
        let home = temp_home();
        let dir = home.join("sessions/2026/06/19");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        fs::write(dir.join("session_index.jsonl"), "{}").unwrap();
        let log = CodexLog::with_home(&home);
        assert_eq!(log.list_sessions().len(), 0);
        let _ = fs::remove_dir_all(&home);
    }
}
