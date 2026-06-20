//! 账号池：Codex（ChatGPT）多账号轮询 + 健康/冷却故障转移 + 手动切换。
//!
//! 设计为「与载体无关」的纯逻辑：只负责"下一次请求该用哪个账号、失败如何降级"，
//! 不做任何网络 I/O，也不依赖 `ferry-auth`。token 的加载与刷新由上层（daemon/ipc）
//! 负责，通过 [`AccountPool::set_accounts`] / [`AccountPool::update_token`] 灌入。
//!
//! 选择策略：
//! - **手动 pin**（点击切换账号）：固定优先该账号，其余账号仍作为故障转移备选。
//! - **轮询开启**（默认）：round-robin，跳过冷却中的账号，健康账号优先。
//! - **轮询关闭**：固定使用当前游标账号，其余作为备选。
//!
//! 故障转移：一次请求拿到 [`AccountPool::attempt_order`] 返回的有序候选列表，
//! 上层逐个尝试，成功 [`AccountPool::mark_success`]、失败 [`AccountPool::mark_failure`]；
//! 连续失败达阈值的账号进入冷却（一段时间内不参与轮询）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;

/// 默认连续失败多少次后进入冷却。
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;
/// 默认冷却时长（秒）。
pub const DEFAULT_COOLDOWN_SECS: u64 = 60;

/// 账号池中的一个上游账号（最小转发信息，不含刷新令牌等敏感材料）。
#[derive(Clone, Debug)]
pub struct PoolAccount {
    /// 唯一键（account_id / email / secret_ref 之一），用于状态跟踪与切换。
    pub key: String,
    /// 展示名（email 优先）。
    pub display_name: String,
    /// 上游基址，默认 Codex 官方 `https://chatgpt.com/backend-api/codex`。
    pub base_url: String,
    /// ChatGPT access token（Bearer）。
    pub access_token: String,
    /// `ChatGPT-Account-ID` 头的值（从 id_token JWT 解析）。
    pub account_id: Option<String>,
    /// 登录方式（`chatgpt` / `apikey`），仅用于展示。
    pub auth_mode: String,
    /// access token 过期 Unix 秒（展示与刷新判断用，可空）。
    pub expires_at: Option<i64>,
}

/// 单个账号的运行时健康状态。
#[derive(Clone, Debug, Default)]
struct Health {
    consecutive_failures: u32,
    total_requests: u64,
    total_failures: u64,
    cooldown_until: Option<Instant>,
    last_error: Option<String>,
}

/// 单个配额窗口（5h 主窗 / 7d 次窗）。字段对齐 Codex `x-codex-*` 响应头。
#[derive(Clone, Debug, Default, Serialize)]
pub struct QuotaWindow {
    /// 已用百分比（0~100）。
    pub used_percent: Option<f64>,
    /// 窗口时长（分钟）。
    pub window_minutes: Option<i64>,
    /// 重置时间（Unix 秒）。
    pub reset_at: Option<i64>,
}

impl QuotaWindow {
    fn is_empty(&self) -> bool {
        self.used_percent.is_none() && self.window_minutes.is_none() && self.reset_at.is_none()
    }
    /// 剩余百分比（无数据时视为满额 100）。
    fn remaining_percent(&self) -> f64 {
        self.used_percent.map(|u| (100.0 - u).max(0.0)).unwrap_or(100.0)
    }
}

/// 一个账号的配额快照（plan + 主/次窗）。
#[derive(Clone, Debug, Default, Serialize)]
pub struct AccountQuota {
    /// 订阅类型：free / plus / team / pro / enterprise。
    pub plan_type: Option<String>,
    /// 5 小时主窗。
    pub primary: QuotaWindow,
    /// 7 天次窗。
    pub secondary: QuotaWindow,
    /// 最近更新时间（Unix 秒）。
    pub updated_at: Option<i64>,
}

impl AccountQuota {
    /// 是否有任何可用配额信息。
    pub fn has_data(&self) -> bool {
        self.plan_type.is_some() || !self.primary.is_empty() || !self.secondary.is_empty()
    }

    /// plan 权重：plan 越高、并发/容量越大，越优先被选中。
    fn plan_weight(&self) -> f64 {
        match self.plan_type.as_deref().map(|s| s.to_ascii_lowercase()) {
            Some(ref p) if p == "enterprise" => 4.0,
            Some(ref p) if p == "team" || p == "business" => 3.0,
            Some(ref p) if p == "pro" => 2.5,
            Some(ref p) if p == "plus" => 2.0,
            Some(ref p) if p == "free" => 1.0,
            _ => 1.5,
        }
    }

    /// 调度评分：剩余主窗配额 × plan 权重（越大越优先）。
    fn score(&self) -> f64 {
        self.primary.remaining_percent() * self.plan_weight()
    }

    /// 主窗是否已耗尽（≥100%）。
    fn primary_exhausted(&self) -> bool {
        self.primary.used_percent.is_some_and(|u| u >= 100.0)
    }
}

/// 账号池调度策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoolStrategy {
    /// 轮询（默认）：round-robin。
    #[default]
    RoundRobin,
    /// 配额感知：优先剩余配额多、plan 高的账号。
    QuotaAware,
}

impl PoolStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::QuotaAware => "quota_aware",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "round_robin" | "roundrobin" | "rr" => Some(Self::RoundRobin),
            "quota_aware" | "quota" | "quotaaware" => Some(Self::QuotaAware),
            _ => None,
        }
    }
}

/// 账号池。
pub struct AccountPool {
    accounts: Vec<PoolAccount>,
    health: HashMap<String, Health>,
    quota: HashMap<String, AccountQuota>,
    cursor: usize,
    pinned: Option<String>,
    rotation_enabled: bool,
    strategy: PoolStrategy,
    cooldown: Duration,
    failure_threshold: u32,
}

impl Default for AccountPool {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl AccountPool {
    /// 用账号列表构造（默认阈值与冷却）。
    pub fn new(accounts: Vec<PoolAccount>) -> Self {
        let mut pool = Self {
            accounts: Vec::new(),
            health: HashMap::new(),
            quota: HashMap::new(),
            cursor: 0,
            pinned: None,
            rotation_enabled: true,
            strategy: PoolStrategy::RoundRobin,
            cooldown: Duration::from_secs(DEFAULT_COOLDOWN_SECS),
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
        };
        pool.set_accounts(accounts);
        pool
    }

    /// 自定义冷却时长与失败阈值。
    pub fn with_policy(mut self, cooldown: Duration, failure_threshold: u32) -> Self {
        self.cooldown = cooldown;
        self.failure_threshold = failure_threshold.max(1);
        self
    }

    /// 重新设置账号列表（保留仍存在账号的健康状态；清理已删除账号；pin 失效则取消）。
    pub fn set_accounts(&mut self, accounts: Vec<PoolAccount>) {
        let keys: Vec<String> = accounts.iter().map(|a| a.key.clone()).collect();
        self.health.retain(|k, _| keys.contains(k));
        self.quota.retain(|k, _| keys.contains(k));
        for a in &accounts {
            self.health.entry(a.key.clone()).or_default();
        }
        if let Some(p) = &self.pinned {
            if !keys.contains(p) {
                self.pinned = None;
            }
        }
        if accounts.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= accounts.len() {
            self.cursor %= accounts.len();
        }
        self.accounts = accounts;
    }

    /// 刷新某账号的 token 与过期时间（token 续期后调用）。
    pub fn update_token(&mut self, key: &str, access_token: String, expires_at: Option<i64>) -> bool {
        if let Some(a) = self.accounts.iter_mut().find(|a| a.key == key) {
            a.access_token = access_token;
            a.expires_at = expires_at;
            true
        } else {
            false
        }
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    pub fn rotation_enabled(&self) -> bool {
        self.rotation_enabled
    }

    /// 开关轮询。关闭后固定使用当前账号（其余仍可作故障转移备选）。
    pub fn set_rotation(&mut self, enabled: bool) {
        self.rotation_enabled = enabled;
    }

    pub fn strategy(&self) -> PoolStrategy {
        self.strategy
    }

    /// 设置调度策略（轮询 / 配额感知）。
    pub fn set_strategy(&mut self, strategy: PoolStrategy) {
        self.strategy = strategy;
    }

    /// 更新某账号的配额快照（被动抓头或主动探测后调用）。
    pub fn update_quota(&mut self, key: &str, quota: AccountQuota) -> bool {
        if self.accounts.iter().any(|a| a.key == key) {
            self.quota.insert(key.to_string(), quota);
            true
        } else {
            false
        }
    }

    /// 读取某账号的配额快照。
    pub fn quota_of(&self, key: &str) -> Option<&AccountQuota> {
        self.quota.get(key)
    }

    /// 克隆全部账号（含 token），供上层做主动配额探测 / 健康检查。
    pub fn accounts_snapshot(&self) -> Vec<PoolAccount> {
        self.accounts.clone()
    }

    pub fn pinned(&self) -> Option<&str> {
        self.pinned.as_deref()
    }

    /// 手动切换（pin）到指定账号；key 不存在返回 false。
    pub fn pin(&mut self, key: &str) -> bool {
        if let Some(idx) = self.index_of(key) {
            self.pinned = Some(self.accounts[idx].key.clone());
            self.cursor = idx;
            true
        } else {
            false
        }
    }

    /// 取消手动 pin，恢复轮询/固定逻辑。
    pub fn unpin(&mut self) {
        self.pinned = None;
    }

    fn index_of(&self, key: &str) -> Option<usize> {
        self.accounts.iter().position(|a| a.key == key)
    }

    fn is_healthy(&self, idx: usize, now: Instant) -> bool {
        let key = &self.accounts[idx].key;
        self.health
            .get(key)
            .and_then(|h| h.cooldown_until)
            .is_none_or(|t| now >= t)
    }

    /// 不改变状态地计算当前候选顺序（下标）。
    fn ordered_indices(&self, now: Instant) -> Vec<usize> {
        let n = self.accounts.len();
        if n == 0 {
            return Vec::new();
        }
        let start = match &self.pinned {
            Some(pk) => self.index_of(pk).unwrap_or(self.cursor % n),
            None => self.cursor % n,
        };
        let mut order: Vec<usize> = (0..n).map(|i| (start + i) % n).collect();
        // pin 模式：固定让 pin 账号排第一（不按健康度重排），其余作备选。
        // 非 pin 模式：健康账号优先（稳定排序保留轮转次序）；
        // QuotaAware 策略下，健康组内再按「剩余配额 × plan 权重」降序。
        if self.pinned.is_none() {
            let quota_aware = self.strategy == PoolStrategy::QuotaAware;
            order.sort_by(|&a, &b| {
                let unhealthy = self.is_healthy(a, now).cmp(&self.is_healthy(b, now)).reverse();
                if !quota_aware {
                    return unhealthy;
                }
                unhealthy.then_with(|| {
                    let sa = self
                        .quota
                        .get(&self.accounts[a].key)
                        .map(AccountQuota::score)
                        .unwrap_or(150.0);
                    let sb = self
                        .quota
                        .get(&self.accounts[b].key)
                        .map(AccountQuota::score)
                        .unwrap_or(150.0);
                    sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
                })
            });
        }
        order
    }

    /// 当前将被选中的账号 key（不改变状态，供快照标注 current）。
    pub fn current_key(&self) -> Option<String> {
        self.ordered_indices(Instant::now())
            .first()
            .map(|&i| self.accounts[i].key.clone())
    }

    /// 取一次请求的有序候选账号（首选 + 故障转移备选），并按策略推进游标。
    pub fn attempt_order(&mut self) -> Vec<PoolAccount> {
        let now = Instant::now();
        let order = self.ordered_indices(now);
        // 仅在「轮询开启且未 pin」时推进游标，让下次换下一个账号。
        if self.rotation_enabled && self.pinned.is_none() && !self.accounts.is_empty() {
            self.cursor = (self.cursor + 1) % self.accounts.len();
        }
        order.into_iter().map(|i| self.accounts[i].clone()).collect()
    }

    /// 标记某账号本次请求成功：清零连续失败与冷却。
    pub fn mark_success(&mut self, key: &str) {
        let h = self.health.entry(key.to_string()).or_default();
        h.consecutive_failures = 0;
        h.cooldown_until = None;
        h.last_error = None;
        h.total_requests += 1;
    }

    /// 标记某账号本次请求失败：累计失败，达阈值则进入冷却。
    pub fn mark_failure(&mut self, key: &str, error: impl Into<String>) {
        let cooldown = self.cooldown;
        let threshold = self.failure_threshold;
        let h = self.health.entry(key.to_string()).or_default();
        h.total_requests += 1;
        h.total_failures += 1;
        h.consecutive_failures += 1;
        h.last_error = Some(error.into());
        if h.consecutive_failures >= threshold {
            h.cooldown_until = Some(Instant::now() + cooldown);
            h.consecutive_failures = 0;
        }
    }

    /// 池状态快照（供管理 API 展示）。
    pub fn snapshot(&self) -> PoolSnapshot {
        let now = Instant::now();
        let current = self.current_key();
        let mut healthy = 0usize;
        let mut cooling = 0usize;
        let accounts = self
            .accounts
            .iter()
            .map(|a| {
                let h = self.health.get(&a.key).cloned().unwrap_or_default();
                let is_cooling = h.cooldown_until.is_some_and(|t| now < t);
                if is_cooling {
                    cooling += 1;
                } else {
                    healthy += 1;
                }
                let cooldown_remaining_secs = h
                    .cooldown_until
                    .and_then(|t| t.checked_duration_since(now))
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let quota = self.quota.get(&a.key).cloned().unwrap_or_default();
                PoolAccountStatus {
                    key: a.key.clone(),
                    display_name: a.display_name.clone(),
                    account_id: a.account_id.clone(),
                    auth_mode: a.auth_mode.clone(),
                    healthy: !is_cooling,
                    cooling_down: is_cooling,
                    cooldown_remaining_secs,
                    consecutive_failures: h.consecutive_failures,
                    total_requests: h.total_requests,
                    total_failures: h.total_failures,
                    token_present: !a.access_token.is_empty(),
                    expires_at: a.expires_at,
                    last_error: h.last_error.clone(),
                    is_current: current.as_deref() == Some(a.key.as_str()),
                    plan_type: quota.plan_type.clone(),
                    primary_used_percent: quota.primary.used_percent,
                    primary_reset_at: quota.primary.reset_at,
                    secondary_used_percent: quota.secondary.used_percent,
                    secondary_reset_at: quota.secondary.reset_at,
                    quota_updated_at: quota.updated_at,
                    quota_exhausted: quota.primary_exhausted(),
                }
            })
            .collect();
        PoolSnapshot {
            total: self.accounts.len(),
            healthy,
            cooling_down: cooling,
            rotation_enabled: self.rotation_enabled,
            strategy: self.strategy.as_str().to_string(),
            pinned: self.pinned.clone(),
            current,
            accounts,
        }
    }
}

/// 账号池状态快照。
#[derive(Clone, Debug, Serialize)]
pub struct PoolSnapshot {
    pub total: usize,
    pub healthy: usize,
    pub cooling_down: usize,
    pub rotation_enabled: bool,
    /// 当前调度策略（round_robin / quota_aware）。
    pub strategy: String,
    pub pinned: Option<String>,
    pub current: Option<String>,
    pub accounts: Vec<PoolAccountStatus>,
}

/// 单账号状态（快照项）。
#[derive(Clone, Debug, Serialize)]
pub struct PoolAccountStatus {
    pub key: String,
    pub display_name: String,
    pub account_id: Option<String>,
    pub auth_mode: String,
    pub healthy: bool,
    pub cooling_down: bool,
    pub cooldown_remaining_secs: u64,
    pub consecutive_failures: u32,
    pub total_requests: u64,
    pub total_failures: u64,
    pub token_present: bool,
    pub expires_at: Option<i64>,
    pub last_error: Option<String>,
    pub is_current: bool,
    /// 订阅类型（free/plus/team/...）。
    pub plan_type: Option<String>,
    /// 5h 主窗已用百分比。
    pub primary_used_percent: Option<f64>,
    /// 5h 主窗重置时间（Unix 秒）。
    pub primary_reset_at: Option<i64>,
    /// 7d 次窗已用百分比。
    pub secondary_used_percent: Option<f64>,
    /// 7d 次窗重置时间（Unix 秒）。
    pub secondary_reset_at: Option<i64>,
    /// 配额最近更新时间（Unix 秒）。
    pub quota_updated_at: Option<i64>,
    /// 主窗是否已耗尽。
    pub quota_exhausted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(key: &str) -> PoolAccount {
        PoolAccount {
            key: key.to_string(),
            display_name: format!("{key}@example.com"),
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
            access_token: format!("token-{key}"),
            account_id: Some(format!("acc-{key}")),
            auth_mode: "chatgpt".to_string(),
            expires_at: None,
        }
    }

    fn pool3() -> AccountPool {
        AccountPool::new(vec![acct("a"), acct("b"), acct("c")])
    }

    fn first_keys(order: &[PoolAccount]) -> Vec<String> {
        order.iter().map(|a| a.key.clone()).collect()
    }

    #[test]
    fn round_robin_advances_each_call() {
        let mut pool = pool3();
        assert_eq!(pool.attempt_order()[0].key, "a");
        assert_eq!(pool.attempt_order()[0].key, "b");
        assert_eq!(pool.attempt_order()[0].key, "c");
        assert_eq!(pool.attempt_order()[0].key, "a", "应回环");
    }

    #[test]
    fn attempt_order_includes_all_as_failover() {
        let mut pool = pool3();
        let order = pool.attempt_order();
        assert_eq!(order.len(), 3, "应返回全部账号作故障转移候选");
        assert_eq!(first_keys(&order), vec!["a", "b", "c"]);
    }

    #[test]
    fn cooldown_skips_account_until_recovered() {
        // 阈值 1：一次失败即冷却。
        let mut pool = pool3().with_policy(Duration::from_secs(300), 1);
        pool.mark_failure("a", "boom");
        // a 冷却中，下一次首选应跳到健康账号（b 或 c），a 落到末尾。
        let order = pool.attempt_order();
        assert_ne!(order[0].key, "a", "冷却账号不应排首位");
        assert_eq!(*order.last().unwrap().key, *"a", "冷却账号应作末位兜底");
    }

    #[test]
    fn cooldown_zero_recovers_immediately() {
        let mut pool = pool3().with_policy(Duration::ZERO, 1);
        pool.mark_failure("a", "boom");
        // 冷却 0 秒：立即恢复健康。
        let snap = pool.snapshot();
        let a = snap.accounts.iter().find(|x| x.key == "a").unwrap();
        assert!(a.healthy, "冷却 0 应立即恢复");
    }

    #[test]
    fn mark_success_clears_failures() {
        let mut pool = pool3().with_policy(Duration::from_secs(300), 3);
        pool.mark_failure("a", "e1");
        pool.mark_failure("a", "e2");
        pool.mark_success("a");
        let snap = pool.snapshot();
        let a = snap.accounts.iter().find(|x| x.key == "a").unwrap();
        assert_eq!(a.consecutive_failures, 0);
        assert!(a.healthy);
        assert_eq!(a.total_requests, 3);
        assert_eq!(a.total_failures, 2);
    }

    #[test]
    fn threshold_triggers_cooldown() {
        let mut pool = pool3().with_policy(Duration::from_secs(300), 3);
        pool.mark_failure("a", "e1");
        pool.mark_failure("a", "e2");
        let snap_before = pool.snapshot();
        assert!(
            snap_before.accounts.iter().find(|x| x.key == "a").unwrap().healthy,
            "未到阈值不应冷却"
        );
        pool.mark_failure("a", "e3");
        let snap_after = pool.snapshot();
        assert!(
            snap_after.accounts.iter().find(|x| x.key == "a").unwrap().cooling_down,
            "达阈值应进入冷却"
        );
    }

    #[test]
    fn pin_fixes_account_but_keeps_failover() {
        let mut pool = pool3();
        assert!(pool.pin("c"));
        assert_eq!(pool.pinned(), Some("c"));
        // pin 后每次首选都是 c，且不推进游标。
        assert_eq!(pool.attempt_order()[0].key, "c");
        assert_eq!(pool.attempt_order()[0].key, "c");
        let order = pool.attempt_order();
        assert_eq!(order.len(), 3, "其余账号仍作故障转移备选");
        pool.unpin();
        assert_eq!(pool.pinned(), None);
    }

    #[test]
    fn pin_unknown_key_fails() {
        let mut pool = pool3();
        assert!(!pool.pin("zzz"));
        assert_eq!(pool.pinned(), None);
    }

    #[test]
    fn rotation_disabled_fixes_current() {
        let mut pool = pool3();
        pool.set_rotation(false);
        assert!(!pool.rotation_enabled());
        // 关闭轮询：固定首选当前游标账号（a），不推进。
        assert_eq!(pool.attempt_order()[0].key, "a");
        assert_eq!(pool.attempt_order()[0].key, "a");
    }

    #[test]
    fn set_accounts_preserves_health_and_clears_removed() {
        let mut pool = pool3().with_policy(Duration::from_secs(300), 1);
        pool.mark_failure("b", "boom"); // b 冷却
        // 重载：去掉 c，保留 a、b。
        pool.set_accounts(vec![acct("a"), acct("b")]);
        let snap = pool.snapshot();
        assert_eq!(snap.total, 2);
        assert!(
            snap.accounts.iter().find(|x| x.key == "b").unwrap().cooling_down,
            "重载应保留 b 的冷却状态"
        );
        assert!(snap.accounts.iter().all(|x| x.key != "c"));
    }

    #[test]
    fn set_accounts_drops_invalid_pin() {
        let mut pool = pool3();
        pool.pin("c");
        pool.set_accounts(vec![acct("a"), acct("b")]);
        assert_eq!(pool.pinned(), None, "pin 的账号被移除后应取消 pin");
    }

    #[test]
    fn update_token_replaces_secret() {
        let mut pool = pool3();
        assert!(pool.update_token("a", "fresh-token".to_string(), Some(123)));
        assert!(!pool.update_token("nope", "x".to_string(), None));
        let order = pool.attempt_order();
        let a = order.iter().find(|x| x.key == "a").unwrap();
        assert_eq!(a.access_token, "fresh-token");
        assert_eq!(a.expires_at, Some(123));
    }

    #[test]
    fn empty_pool_yields_no_candidates() {
        let mut pool = AccountPool::default();
        assert!(pool.is_empty());
        assert!(pool.attempt_order().is_empty());
        assert!(pool.current_key().is_none());
        let snap = pool.snapshot();
        assert_eq!(snap.total, 0);
        assert_eq!(snap.healthy, 0);
    }

    #[test]
    fn all_cooling_still_returns_candidates() {
        let mut pool = pool3().with_policy(Duration::from_secs(300), 1);
        pool.mark_failure("a", "e");
        pool.mark_failure("b", "e");
        pool.mark_failure("c", "e");
        // 全部冷却时仍应返回候选（兜底，否则无法服务）。
        let order = pool.attempt_order();
        assert_eq!(order.len(), 3);
        let snap = pool.snapshot();
        assert_eq!(snap.cooling_down, 3);
        assert_eq!(snap.healthy, 0);
    }

    #[test]
    fn snapshot_marks_current() {
        let pool = pool3();
        let snap = pool.snapshot();
        assert_eq!(snap.current.as_deref(), Some("a"));
        let current_count = snap.accounts.iter().filter(|x| x.is_current).count();
        assert_eq!(current_count, 1);
        assert!(snap.accounts.iter().find(|x| x.key == "a").unwrap().is_current);
    }

    fn quota(plan: &str, primary_used: f64) -> AccountQuota {
        AccountQuota {
            plan_type: Some(plan.to_string()),
            primary: QuotaWindow {
                used_percent: Some(primary_used),
                window_minutes: Some(300),
                reset_at: Some(1_900_000_000),
            },
            secondary: QuotaWindow {
                used_percent: Some(10.0),
                window_minutes: Some(10080),
                reset_at: Some(1_900_500_000),
            },
            updated_at: Some(1),
        }
    }

    #[test]
    fn update_quota_only_for_known_accounts() {
        let mut pool = pool3();
        assert!(pool.update_quota("a", quota("plus", 20.0)));
        assert!(!pool.update_quota("zzz", quota("plus", 1.0)));
        let snap = pool.snapshot();
        let a = snap.accounts.iter().find(|x| x.key == "a").unwrap();
        assert_eq!(a.plan_type.as_deref(), Some("plus"));
        assert_eq!(a.primary_used_percent, Some(20.0));
        assert_eq!(a.secondary_used_percent, Some(10.0));
        assert!(!a.quota_exhausted);
    }

    #[test]
    fn quota_aware_prefers_more_remaining_and_higher_plan() {
        let mut pool = pool3();
        pool.set_strategy(PoolStrategy::QuotaAware);
        // a: plus 90% 用尽（剩 10）; b: plus 5% 用（剩 95）; c: free 5% 用（剩 95）。
        pool.update_quota("a", quota("plus", 90.0));
        pool.update_quota("b", quota("plus", 5.0));
        pool.update_quota("c", quota("free", 5.0));
        let order = pool.attempt_order();
        // b（plus 剩95）应排第一，c（free 剩95）次之，a（plus 剩10）末位。
        assert_eq!(order[0].key, "b");
        assert_eq!(order[1].key, "c");
        assert_eq!(order[2].key, "a");
    }

    #[test]
    fn round_robin_default_ignores_quota() {
        let mut pool = pool3();
        pool.update_quota("a", quota("free", 99.0));
        // 默认 round-robin：仍从 a 开始，不因配额重排。
        assert_eq!(pool.strategy(), PoolStrategy::RoundRobin);
        assert_eq!(pool.attempt_order()[0].key, "a");
    }

    #[test]
    fn strategy_parse_roundtrip() {
        assert_eq!(PoolStrategy::parse("quota_aware"), Some(PoolStrategy::QuotaAware));
        assert_eq!(PoolStrategy::parse("rr"), Some(PoolStrategy::RoundRobin));
        assert_eq!(PoolStrategy::parse("nope"), None);
        assert_eq!(PoolStrategy::QuotaAware.as_str(), "quota_aware");
    }

    #[test]
    fn set_accounts_retains_quota_for_kept_accounts() {
        let mut pool = pool3();
        pool.update_quota("a", quota("plus", 30.0));
        pool.set_accounts(vec![acct("a"), acct("b")]);
        assert!(pool.quota_of("a").is_some());
        assert!(pool.quota_of("c").is_none());
    }
}
