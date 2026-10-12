//! 通用重试工具模块。
//!
//! 提供指数退避重试策略，适用于网络请求等易受瞬时故障影响的操作。
//! 可通过环境变量配置：
//! - `LLAMAUI_MAX_RETRIES`：最大重试次数（默认 3）
//! - `LLAMAUI_RETRY_BASE_MS`：基础退避毫秒数（默认 500）

#![allow(clippy::module_name_repetitions)]

use std::env;
use tracing::warn;

/// 获取最大重试次数（可从环境变量配置）。
pub fn max_retries() -> u32 {
    env::var("LLAMAUI_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

/// 获取基础退避毫秒数（可从环境变量配置）。
pub fn retry_base_ms() -> u64 {
    env::var("LLAMAUI_RETRY_BASE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500)
}

/// 指数退避重试。
///
/// 对闭包 `f` 反复调用，直到返回 `Ok` 或达到最大重试次数。
/// 每次失败后等待 `base_ms * 2^(attempt-1)` 毫秒（上限 30 秒）。
///
/// # 参数
/// - `context`：描述性文本，用于日志和错误信息
/// - `f`：要重试的操作
/// - `max_retries`：最大重试次数（含首次尝试）
/// - `base_ms`：基础退避毫秒数
pub async fn retry<F, Fut, T, E>(
    context: &str,
    mut f: F,
    max_retries: u32,
    base_ms: u64,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    for attempt in 1..=max_retries {
        match f().await {
            Ok(val) => {
                if attempt > 1 {
                    tracing::info!(target: "Retry", context, attempt, "重试成功");
                }
                return Ok(val);
            }
            Err(e) => {
                if attempt >= max_retries {
                    warn!(target: "Retry", context, attempt, max_retries, "所有重试均失败");
                    return Err(e);
                }
                let delay_ms = (base_ms as u64)
                    .saturating_mul(2u64.saturating_pow(attempt - 1))
                    .min(30_000);
                warn!(target: "Retry", context, attempt, delay_ms, "重试失败，等待后重试");
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
        }
    }

    unreachable!("for loop should have returned by now")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_retry_success_on_first_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let result = retry(
            "test",
            {
                let calls = Arc::clone(&calls);
                move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(42)
                    }
                }
            },
            3,
            10,
        )
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let result = retry(
            "test",
            {
                let calls = Arc::clone(&calls);
                move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                        if n < 3 {
                            Err("fail")
                        } else {
                            Ok(())
                        }
                    }
                }
            },
            5,
            10,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let calls = Arc::new(AtomicUsize::new(0));
        let result: Result<(), &str> = retry(
            "test",
            {
                let calls = Arc::clone(&calls);
                move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Err("always fail")
                    }
                }
            },
            3,
            10,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
