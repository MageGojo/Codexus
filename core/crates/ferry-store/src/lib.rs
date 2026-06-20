//! ferry-store：码渡本地 SQLite 存储层。
//!
//! M1 阶段先覆盖会话记录：请求、响应、模型、用量、耗时与状态。
//! 后续 `ferry-proxy` 可在请求结束时把 [`NewSessionRecord`] 写入这里。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一次代理会话的最终状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Succeeded,
    Failed,
}

impl SessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "failed" => Self::Failed,
            _ => Self::Succeeded,
        }
    }
}

/// 写入新会话时的输入结构。
#[derive(Debug, Clone)]
pub struct NewSessionRecord {
    pub provider: String,
    /// 实际承载本次请求的账号稳定 id（供按账号统计 token 用量；可空）。
    pub account: String,
    pub upstream_api: String,
    pub requested_model: String,
    pub target_model: String,
    pub stream: bool,
    pub status: SessionStatus,
    pub duration_ms: i64,
    /// 上游 usage 上报的 token（中转可能虚报/掺假）。
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    /// 码渡本地分词器独立估算的 token（用于与上游对比、识别掺假）。
    pub est_input_tokens: i64,
    pub est_output_tokens: i64,
    pub est_total_tokens: i64,
    pub error: Option<String>,
    pub request_json: Value,
    pub response_json: Option<Value>,
    pub output_text: Option<String>,
}

/// 已落库的会话记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub provider: String,
    /// 实际承载本次请求的账号稳定 id（按账号统计用；旧记录为空）。
    #[serde(default)]
    pub account: String,
    pub upstream_api: String,
    pub requested_model: String,
    pub target_model: String,
    pub stream: bool,
    pub status: SessionStatus,
    pub duration_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    #[serde(default)]
    pub est_input_tokens: i64,
    #[serde(default)]
    pub est_output_tokens: i64,
    #[serde(default)]
    pub est_total_tokens: i64,
    pub error: Option<String>,
    pub request_json: Value,
    pub response_json: Option<Value>,
    pub output_text: Option<String>,
}

/// 按供应商/账号聚合的 token 用量（含上游上报与本地估算，用于掺假对比）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    /// 会话里的 provider 标识（供应商 base_url，或 `codex-pool:<账号>`）。
    pub provider: String,
    pub requests: i64,
    pub reported_total: i64,
    pub reported_input: i64,
    pub reported_output: i64,
    pub est_total: i64,
    pub est_input: i64,
    pub est_output: i64,
}

/// 按账号聚合的 token 用量（用于账号卡片悬浮显示「该账号用了多少 token」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountUsage {
    /// 账号稳定 id（与 `ferry_ipc` 的 account id 一致）。
    pub account: String,
    pub requests: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 某天的会话聚合（token / 请求 / 成败）。
#[derive(Debug, Clone)]
pub struct DayTokens {
    /// `YYYY-MM-DD`（UTC）。
    pub date: String,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub requests: i64,
    pub succeeded: i64,
    pub failed: i64,
}

/// 某天的账号增删聚合。
#[derive(Debug, Clone)]
pub struct DayAccounts {
    pub date: String,
    pub added: i64,
    pub deleted: i64,
}

/// 全量会话总计。
#[derive(Debug, Clone, Default)]
pub struct SessionTotals {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub requests: i64,
    pub succeeded: i64,
    pub failed: i64,
}

/// SQLite 存储句柄。
pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    /// 打开或创建数据库，并执行迁移。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建存储目录 {} 失败", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("打开 SQLite 数据库 {} 失败", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// 定位默认数据库：优先 `FERRY_STORE_DB`，否则 `~/.codexferry/codexferry.db`。
    pub fn locate() -> Result<Self> {
        let path = match std::env::var_os("FERRY_STORE_DB") {
            Some(p) => PathBuf::from(p),
            None => home_dir()?.join(".codexferry").join("codexferry.db"),
        };
        Self::open(path)
    }

    /// 测试或临时场景使用内存数据库。
    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            conn: Connection::open_in_memory().context("打开内存 SQLite 数据库失败")?,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        // 第一步：仅建表（不建依赖「后加列」的索引）。新库一次到位，旧库走下面的 ALTER 补列。
        self.conn
            .execute_batch(
                r#"
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    created_at TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    account TEXT NOT NULL DEFAULT '',
                    upstream_api TEXT NOT NULL,
                    requested_model TEXT NOT NULL,
                    target_model TEXT NOT NULL,
                    stream INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    duration_ms INTEGER NOT NULL DEFAULT 0,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    est_input_tokens INTEGER NOT NULL DEFAULT 0,
                    est_output_tokens INTEGER NOT NULL DEFAULT 0,
                    est_total_tokens INTEGER NOT NULL DEFAULT 0,
                    error TEXT,
                    request_json TEXT NOT NULL,
                    response_json TEXT,
                    output_text TEXT
                );

                CREATE TABLE IF NOT EXISTS account_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts TEXT NOT NULL,
                    event TEXT NOT NULL,
                    account_key TEXT NOT NULL,
                    auth_mode TEXT NOT NULL DEFAULT ''
                );
                "#,
            )
            .context("执行 SQLite 迁移失败（建表）")?;

        // 第二步：旧库补列。必须在「建索引」之前执行，否则 idx_sessions_account 等会因
        // 「no such column」整批失败（旧库的 sessions 表可能没有 account / est_* 列）。
        // 重复执行报「duplicate column」，忽略即可。
        for stmt in [
            "ALTER TABLE sessions ADD COLUMN est_input_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN est_output_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN est_total_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN account TEXT NOT NULL DEFAULT ''",
        ] {
            let _ = self.conn.execute(stmt, []);
        }

        // 第三步：列齐全后再建索引（此时 account / est_* 一定存在）。
        self.conn
            .execute_batch(
                r#"
                CREATE INDEX IF NOT EXISTS idx_sessions_created_at
                    ON sessions(created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_sessions_status
                    ON sessions(status);
                CREATE INDEX IF NOT EXISTS idx_sessions_model
                    ON sessions(target_model);
                CREATE INDEX IF NOT EXISTS idx_sessions_provider
                    ON sessions(provider);
                CREATE INDEX IF NOT EXISTS idx_sessions_account
                    ON sessions(account);

                CREATE INDEX IF NOT EXISTS idx_account_events_ts
                    ON account_events(ts DESC);
                CREATE INDEX IF NOT EXISTS idx_account_events_event
                    ON account_events(event);
                "#,
            )
            .context("执行 SQLite 迁移失败（建索引）")?;
        Ok(())
    }

    /// 插入一条完整会话记录，返回生成的会话 ID。
    pub fn insert_session(&self, record: &NewSessionRecord) -> Result<String> {
        let id = format!("sess_{}", uuid::Uuid::new_v4().simple());
        let created_at = Utc::now();
        self.conn
            .execute(
                r#"
                INSERT INTO sessions (
                    id, created_at, provider, account, upstream_api, requested_model, target_model,
                    stream, status, duration_ms, input_tokens, output_tokens, total_tokens,
                    est_input_tokens, est_output_tokens, est_total_tokens,
                    error, request_json, response_json, output_text
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                "#,
                params![
                    id,
                    created_at.to_rfc3339(),
                    record.provider,
                    record.account,
                    record.upstream_api,
                    record.requested_model,
                    record.target_model,
                    bool_to_i64(record.stream),
                    record.status.as_str(),
                    record.duration_ms,
                    record.input_tokens,
                    record.output_tokens,
                    record.total_tokens,
                    record.est_input_tokens,
                    record.est_output_tokens,
                    record.est_total_tokens,
                    record.error,
                    serde_json::to_string(&record.request_json)?,
                    optional_json(&record.response_json)?,
                    record.output_text,
                ],
            )
            .context("写入会话记录失败")?;
        Ok(id)
    }

    /// 按 ID 获取会话详情。
    pub fn get_session(&self, id: &str) -> Result<Option<SessionRecord>> {
        self.conn
            .query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                params![id],
                row_to_session,
            )
            .optional()
            .context("查询会话详情失败")
    }

    /// 查询最近会话，默认调用方应传入较小 limit 供 GUI 列表展示。
    pub fn recent_sessions(&self, limit: usize) -> Result<Vec<SessionRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM sessions ORDER BY created_at DESC LIMIT ?1")
            .context("准备最近会话查询失败")?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_session)
            .context("执行最近会话查询失败")?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ---- 账号事件（用于仪表盘统计）----

    /// 记录一条账号事件（ts=now）。`event` 取 `added` / `deleted`。
    pub fn record_account_event(
        &self,
        event: &str,
        account_key: &str,
        auth_mode: &str,
    ) -> Result<()> {
        self.record_account_event_at(event, account_key, auth_mode, &Utc::now().to_rfc3339())
    }

    /// 记录一条账号事件并指定时间（存量回填用）。
    pub fn record_account_event_at(
        &self,
        event: &str,
        account_key: &str,
        auth_mode: &str,
        ts: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO account_events (ts, event, account_key, auth_mode) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![ts, event, account_key, auth_mode],
            )
            .context("写入账号事件失败")?;
        Ok(())
    }

    /// 已有 `added` 事件的账号键集合（回填去重用）。
    pub fn account_added_keys(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT account_key FROM account_events WHERE event = 'added'")
            .context("准备账号键查询失败")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .context("执行账号键查询失败")?;
        let mut set = std::collections::HashSet::new();
        for row in rows {
            set.insert(row?);
        }
        Ok(set)
    }

    /// 累计新增 / 删除事件数。
    pub fn account_event_totals(&self) -> Result<(i64, i64)> {
        let added: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM account_events WHERE event = 'added'",
                [],
                |r| r.get(0),
            )
            .context("统计新增事件失败")?;
        let deleted: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM account_events WHERE event = 'deleted'",
                [],
                |r| r.get(0),
            )
            .context("统计删除事件失败")?;
        Ok((added, deleted))
    }

    /// 全量会话总计。
    pub fn session_totals(&self) -> Result<SessionTotals> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0), \
                 COALESCE(SUM(output_tokens),0), COUNT(*), \
                 COALESCE(SUM(CASE WHEN status='succeeded' THEN 1 ELSE 0 END),0), \
                 COALESCE(SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END),0) FROM sessions",
                [],
                |r| {
                    Ok(SessionTotals {
                        total_tokens: r.get(0)?,
                        input_tokens: r.get(1)?,
                        output_tokens: r.get(2)?,
                        requests: r.get(3)?,
                        succeeded: r.get(4)?,
                        failed: r.get(5)?,
                    })
                },
            )
            .context("统计会话总计失败")
    }

    /// 按 provider 聚合 token 用量（上游上报 vs 本地估算），用于「中转掺假」对比。
    /// 仅统计成功会话；按上报总量降序。
    pub fn provider_usage(&self) -> Result<Vec<ProviderUsage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT provider, COUNT(*), \
                 COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0), \
                 COALESCE(SUM(output_tokens),0), COALESCE(SUM(est_total_tokens),0), \
                 COALESCE(SUM(est_input_tokens),0), COALESCE(SUM(est_output_tokens),0) \
                 FROM sessions WHERE status='succeeded' \
                 GROUP BY provider ORDER BY SUM(total_tokens) DESC",
            )
            .context("准备供应商用量聚合查询失败")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProviderUsage {
                    provider: r.get(0)?,
                    requests: r.get(1)?,
                    reported_total: r.get(2)?,
                    reported_input: r.get(3)?,
                    reported_output: r.get(4)?,
                    est_total: r.get(5)?,
                    est_input: r.get(6)?,
                    est_output: r.get(7)?,
                })
            })
            .context("执行供应商用量聚合查询失败")?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 按账号聚合 token 用量（仅统计成功会话、有账号标识的记录）。
    /// 用于账号卡片悬浮显示「该账号用了多少 token」。
    pub fn account_usage(&self) -> Result<Vec<AccountUsage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT account, COUNT(*), \
                 COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0), \
                 COALESCE(SUM(output_tokens),0) \
                 FROM sessions WHERE status='succeeded' AND account <> '' \
                 GROUP BY account ORDER BY SUM(total_tokens) DESC",
            )
            .context("准备账号用量聚合查询失败")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AccountUsage {
                    account: r.get(0)?,
                    requests: r.get(1)?,
                    total_tokens: r.get(2)?,
                    input_tokens: r.get(3)?,
                    output_tokens: r.get(4)?,
                })
            })
            .context("执行账号用量聚合查询失败")?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 按天聚合的会话序列（`since_day` 为 `YYYY-MM-DD` 下界，含当天）。
    pub fn daily_token_series(&self, since_day: &str) -> Result<Vec<DayTokens>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT substr(created_at,1,10) AS day, COALESCE(SUM(total_tokens),0), \
                 COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COUNT(*), \
                 COALESCE(SUM(CASE WHEN status='succeeded' THEN 1 ELSE 0 END),0), \
                 COALESCE(SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END),0) \
                 FROM sessions WHERE substr(created_at,1,10) >= ?1 GROUP BY day ORDER BY day ASC",
            )
            .context("准备会话日聚合查询失败")?;
        let rows = stmt
            .query_map(params![since_day], |r| {
                Ok(DayTokens {
                    date: r.get(0)?,
                    total_tokens: r.get(1)?,
                    input_tokens: r.get(2)?,
                    output_tokens: r.get(3)?,
                    requests: r.get(4)?,
                    succeeded: r.get(5)?,
                    failed: r.get(6)?,
                })
            })
            .context("执行会话日聚合查询失败")?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 按天聚合的账号增删序列。
    pub fn daily_account_series(&self, since_day: &str) -> Result<Vec<DayAccounts>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT substr(ts,1,10) AS day, \
                 COALESCE(SUM(CASE WHEN event='added' THEN 1 ELSE 0 END),0), \
                 COALESCE(SUM(CASE WHEN event='deleted' THEN 1 ELSE 0 END),0) \
                 FROM account_events WHERE substr(ts,1,10) >= ?1 GROUP BY day ORDER BY day ASC",
            )
            .context("准备账号日聚合查询失败")?;
        let rows = stmt
            .query_map(params![since_day], |r| {
                Ok(DayAccounts {
                    date: r.get(0)?,
                    added: r.get(1)?,
                    deleted: r.get(2)?,
                })
            })
            .context("执行账号日聚合查询失败")?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let created_at: String = row.get("created_at")?;
    let status: String = row.get("status")?;
    let request_json: String = row.get("request_json")?;
    let response_json: Option<String> = row.get("response_json")?;

    Ok(SessionRecord {
        id: row.get("id")?,
        created_at: parse_time(&created_at)?,
        provider: row.get("provider")?,
        account: row.get("account").unwrap_or_default(),
        upstream_api: row.get("upstream_api")?,
        requested_model: row.get("requested_model")?,
        target_model: row.get("target_model")?,
        stream: row.get::<_, i64>("stream")? != 0,
        status: SessionStatus::from_str(&status),
        duration_ms: row.get("duration_ms")?,
        input_tokens: row.get("input_tokens")?,
        output_tokens: row.get("output_tokens")?,
        total_tokens: row.get("total_tokens")?,
        est_input_tokens: row.get("est_input_tokens")?,
        est_output_tokens: row.get("est_output_tokens")?,
        est_total_tokens: row.get("est_total_tokens")?,
        error: row.get("error")?,
        request_json: parse_json(&request_json)?,
        response_json: match response_json {
            Some(s) => Some(parse_json(&s)?),
            None => None,
        },
        output_text: row.get("output_text")?,
    })
}

fn bool_to_i64(v: bool) -> i64 {
    if v {
        1
    } else {
        0
    }
}

fn optional_json(value: &Option<Value>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("序列化 JSON 字段失败")
}

fn parse_json(s: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn parse_time(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("无法确定用户主目录（HOME / USERPROFILE 均未设置）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn insert_and_get_session() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = store.insert_session(&sample_record("hello")).unwrap();

        let got = store.get_session(&id).unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.provider, "deepseek");
        assert_eq!(got.target_model, "deepseek-chat");
        assert_eq!(got.status, SessionStatus::Succeeded);
        assert_eq!(got.output_text.as_deref(), Some("hello"));
        assert_eq!(got.total_tokens, 8);
        assert_eq!(got.request_json["model"], "gpt-5");
        assert_eq!(got.response_json.unwrap()["output_text"], "hello");
    }

    #[test]
    fn recent_sessions_returns_newest_first() {
        let store = SessionStore::open_in_memory().unwrap();
        let older = store.insert_session(&sample_record("old")).unwrap();
        let newer = store.insert_session(&sample_record("new")).unwrap();

        let recent = store.recent_sessions(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, newer);
        assert_eq!(recent[1].id, older);
    }

    #[test]
    fn stores_failed_session_with_error() {
        let store = SessionStore::open_in_memory().unwrap();
        let mut record = sample_record("");
        record.status = SessionStatus::Failed;
        record.error = Some("上游返回 401".to_string());
        record.response_json = None;

        let id = store.insert_session(&record).unwrap();
        let got = store.get_session(&id).unwrap().unwrap();
        assert_eq!(got.status, SessionStatus::Failed);
        assert_eq!(got.error.as_deref(), Some("上游返回 401"));
        assert!(got.response_json.is_none());
    }

    fn sample_record(output: &str) -> NewSessionRecord {
        NewSessionRecord {
            provider: "deepseek".to_string(),
            account: String::new(),
            upstream_api: "chat".to_string(),
            requested_model: "gpt-5".to_string(),
            target_model: "deepseek-chat".to_string(),
            stream: false,
            status: SessionStatus::Succeeded,
            duration_ms: 123,
            input_tokens: 3,
            output_tokens: 5,
            total_tokens: 8,
            est_input_tokens: 3,
            est_output_tokens: 5,
            est_total_tokens: 8,
            error: None,
            request_json: json!({"model":"gpt-5","input":"hi"}),
            response_json: Some(json!({"output_text": output})),
            output_text: Some(output.to_string()),
        }
    }

    #[test]
    fn provider_usage_groups_reported_vs_estimated() {
        let store = SessionStore::open_in_memory().unwrap();
        // 中转 provider：上游上报远高于本地估算（疑似掺假）。
        let mut relay = sample_record("hi");
        relay.provider = "https://relay.example.com/v1".to_string();
        relay.total_tokens = 1000;
        relay.input_tokens = 400;
        relay.output_tokens = 600;
        relay.est_total_tokens = 100;
        relay.est_input_tokens = 40;
        relay.est_output_tokens = 60;
        store.insert_session(&relay).unwrap();

        let usage = store.provider_usage().unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].provider, "https://relay.example.com/v1");
        assert_eq!(usage[0].reported_total, 1000);
        assert_eq!(usage[0].est_total, 100);
        assert_eq!(usage[0].requests, 1);
    }

    #[test]
    fn account_usage_groups_by_account() {
        let store = SessionStore::open_in_memory().unwrap();
        // 两条 alice、一条 bob、一条无账号（不计入）、一条失败（不计入）。
        let mut a1 = sample_record("hi");
        a1.account = "deepseek-alice".into();
        a1.total_tokens = 100;
        a1.input_tokens = 40;
        a1.output_tokens = 60;
        store.insert_session(&a1).unwrap();
        let mut a2 = sample_record("yo");
        a2.account = "deepseek-alice".into();
        a2.total_tokens = 50;
        store.insert_session(&a2).unwrap();
        let mut b1 = sample_record("hey");
        b1.account = "deepseek-bob".into();
        b1.total_tokens = 30;
        store.insert_session(&b1).unwrap();
        store.insert_session(&sample_record("anon")).unwrap(); // account=''
        let mut fail = sample_record("");
        fail.account = "deepseek-alice".into();
        fail.status = SessionStatus::Failed;
        fail.total_tokens = 999;
        store.insert_session(&fail).unwrap();

        let usage = store.account_usage().unwrap();
        assert_eq!(usage.len(), 2, "仅 alice / bob 两个有账号的成功会话");
        let alice = usage.iter().find(|u| u.account == "deepseek-alice").unwrap();
        assert_eq!(alice.requests, 2);
        assert_eq!(alice.total_tokens, 150, "失败会话不计入");
        let bob = usage.iter().find(|u| u.account == "deepseek-bob").unwrap();
        assert_eq!(bob.total_tokens, 30);
    }

    #[test]
    fn account_events_record_and_totals() {
        let store = SessionStore::open_in_memory().unwrap();
        store.record_account_event("added", "a@x.com", "chatgpt").unwrap();
        store.record_account_event("added", "b@x.com", "apikey").unwrap();
        store
            .record_account_event("deleted", "a@x.com", "chatgpt")
            .unwrap();

        let (added, deleted) = store.account_event_totals().unwrap();
        assert_eq!(added, 2);
        assert_eq!(deleted, 1);

        let keys = store.account_added_keys().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("a@x.com"));
        assert!(keys.contains("b@x.com"));
    }

    #[test]
    fn session_totals_and_daily_series() {
        let store = SessionStore::open_in_memory().unwrap();
        store.insert_session(&sample_record("hi")).unwrap();
        let mut fail = sample_record("");
        fail.status = SessionStatus::Failed;
        fail.total_tokens = 4;
        fail.input_tokens = 2;
        fail.output_tokens = 2;
        store.insert_session(&fail).unwrap();

        let totals = store.session_totals().unwrap();
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.succeeded, 1);
        assert_eq!(totals.failed, 1);
        assert_eq!(totals.total_tokens, 12);

        let series = store.daily_token_series("2000-01-01").unwrap();
        let sum_req: i64 = series.iter().map(|d| d.requests).sum();
        let sum_tok: i64 = series.iter().map(|d| d.total_tokens).sum();
        assert_eq!(sum_req, 2);
        assert_eq!(sum_tok, 12);
    }

    #[test]
    fn migrates_legacy_db_without_account_or_est_columns() {
        // 模拟旧库：sessions 表只有早期列（无 account / est_* / idx_sessions_account），
        // 复现「执行 SQLite 迁移失败」的真实场景，验证再次打开能平滑升级。
        let path = std::env::temp_dir().join(format!(
            "ferry_store_legacy_{}_{}.db",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    created_at TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    upstream_api TEXT NOT NULL,
                    requested_model TEXT NOT NULL,
                    target_model TEXT NOT NULL,
                    stream INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    duration_ms INTEGER NOT NULL DEFAULT 0,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    error TEXT,
                    request_json TEXT NOT NULL,
                    response_json TEXT,
                    output_text TEXT
                );
                CREATE INDEX idx_sessions_created_at ON sessions(created_at DESC);
                CREATE INDEX idx_sessions_provider ON sessions(provider);
                "#,
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id, created_at, provider, upstream_api, requested_model, \
                 target_model, stream, status, total_tokens, request_json) \
                 VALUES ('old1', ?1, 'deepseek', 'chat', 'gpt-5', 'deepseek-chat', 0, 'succeeded', 8, '{}')",
                params![Utc::now().to_rfc3339()],
            )
            .unwrap();
        }

        // 打开即触发 migrate()：旧场景下会因 CREATE INDEX ON sessions(account) 失败，修复后应成功。
        let store = SessionStore::open(&path).expect("旧库迁移应成功");

        // account 列已补齐：可按账号写入并聚合。
        let mut rec = sample_record("hi");
        rec.account = "deepseek-alice".into();
        rec.total_tokens = 20;
        store.insert_session(&rec).unwrap();
        let usage = store.account_usage().unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].account, "deepseek-alice");
        // 旧记录仍可读，且 provider 聚合可用。
        assert_eq!(store.recent_sessions(10).unwrap().len(), 2);
        assert!(!store.provider_usage().unwrap().is_empty());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}-wal", path.display()));
        let _ = fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn daily_account_series_buckets_by_day() {
        let store = SessionStore::open_in_memory().unwrap();
        store
            .record_account_event_at("added", "a", "chatgpt", "2026-06-01T10:00:00+00:00")
            .unwrap();
        store
            .record_account_event_at("added", "b", "chatgpt", "2026-06-01T11:00:00+00:00")
            .unwrap();
        store
            .record_account_event_at("deleted", "a", "chatgpt", "2026-06-02T09:00:00+00:00")
            .unwrap();

        let series = store.daily_account_series("2026-06-01").unwrap();
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].date, "2026-06-01");
        assert_eq!(series[0].added, 2);
        assert_eq!(series[0].deleted, 0);
        assert_eq!(series[1].date, "2026-06-02");
        assert_eq!(series[1].deleted, 1);

        let later = store.daily_account_series("2026-06-02").unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].date, "2026-06-02");
    }
}
