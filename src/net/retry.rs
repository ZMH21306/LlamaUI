//! 指数退避 + jitter 重试策略。
//!
//! 用于网络层在请求失败或收到 5xx / 429 时自动重试。

use std::time::Duration;

/// 重试策略（可插拔）。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub jitter: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(8),
            jitter: 0.2,
        }
    }
}

impl RetryPolicy {
    /// 构造重试策略。
    pub fn new(max_attempts: u32) -> Self {
        Self { max_attempts, ..Self::default() }
    }

    /// 计算第 `attempt` 次重试的延迟（指数退避 + jitter）。
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exp = self.base_delay.saturating_mul(1u32.saturating_add(attempt - 1));
        let delay = exp.min(self.max_delay);
        if self.jitter > 0.0 {
            let span = delay.as_millis() as f64 * self.jitter;
            if span > 0.0 {
                let offset = (span * 0.5) as u64;
                let base = delay.as_millis() as u64;
                return Duration::from_millis(base.saturating_sub(offset));
            }
        }
        delay
    }
}
