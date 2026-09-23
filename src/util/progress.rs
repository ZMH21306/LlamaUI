//! 通用下载进度报告器。
//!
//! 为更新下载、HF 模型下载等场景提供：
//! - 字节级下载速度计算（滑动窗口）
//! - ETA 估算
//! - 去抖合并（避免高频事件刷屏）

use std::time::{Duration, Instant};

/// 进度采样点。
#[derive(Debug, Clone)]
struct Sample {
    downloaded: u64,
    instant: Instant,
}

/// 通用下载进度报告器。
///
/// 用法：在下载循环中每收到一个 chunk 调用 `observe`，达到最小间隔时
/// 自动 emit 进度事件（通过 `emit` 回调）。
pub struct ProgressReporter {
    samples: Vec<Sample>,
    window_size: usize,
    min_interval: Duration,
    last_emit: Instant,
    total: u64,
    started_at: Instant,
}

impl ProgressReporter {
    /// 创建报告器。
    ///
    /// - `total`: 预期总字节数（0 表示未知）
    /// - `window_size`: 滑动窗口采样点数
    /// - `min_interval`: 最小发射间隔
    pub fn new(total: u64, window_size: usize, min_interval: Duration) -> Self {
        Self {
            samples: Vec::with_capacity(window_size),
            window_size,
            min_interval,
            last_emit: Instant::now(),
            total,
            started_at: Instant::now(),
        }
    }

    /// 记录一个 chunk 的到达。达到去抖间隔时自动 emit。
    ///
    /// 返回 `(progress, downloaded, speed_bps, eta_secs)` 供调用方 emit。
    pub fn observe(&mut self, downloaded: u64) -> Option<(f64, u64, f64, Option<u64>)> {
        let now = Instant::now();
        self.samples.push(Sample {
            downloaded,
            instant: now,
        });
        if self.samples.len() > self.window_size {
            self.samples.remove(0);
        }

        if now.duration_since(self.last_emit) < self.min_interval {
            return None;
        }
        self.last_emit = now;

        Some(self.compute(downloaded))
    }

    fn compute(&self, downloaded: u64) -> (f64, u64, f64, Option<u64>) {
        let progress = if self.total > 0 {
            downloaded as f64 / self.total as f64
        } else {
            0.0
        };

        let speed_bps = if self.samples.len() >= 2 {
            let first = &self.samples[0];
            let last = self.samples.last().unwrap();
            let elapsed = last.instant.duration_since(first.instant).as_secs_f64();
            if elapsed > 0.0 {
                (last.downloaded - first.downloaded) as f64 / elapsed
            } else {
                0.0
            }
        } else {
            0.0
        };

        let eta_secs = if speed_bps > 0.0 && self.total > downloaded {
            Some(((self.total - downloaded) as f64 / speed_bps) as u64)
        } else {
            None
        };

        (progress, downloaded, speed_bps, eta_secs)
    }

    /// 格式化速度为 MB/s。
    pub fn format_speed(bps: f64) -> String {
        let mbps = bps / 1_048_576.0;
        format!("{:.1} MB/s", mbps)
    }

    /// 格式化字节数为人类可读。
    pub fn format_bytes(bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
        let mut i = 0;
        let mut s = bytes as f64;
        while s >= 1024.0 && i < UNITS.len() - 1 {
            s /= 1024.0;
            i += 1;
        }
        format!("{:.1} {}", s, UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn progress_reporter_computes_speed() {
        let mut reporter = ProgressReporter::new(1024 * 1024, 3, Duration::from_millis(0));
        reporter.observe(0);
        thread::sleep(Duration::from_millis(10));
        reporter.observe(512 * 1024);
        let (progress, downloaded, speed, _) = reporter.observe(1024 * 1024).unwrap();
        assert_eq!(progress, 1.0);
        assert_eq!(downloaded, 1024 * 1024);
        assert!(speed > 0.0);
    }

    #[test]
    fn progress_reporter_debounces() {
        let mut reporter = ProgressReporter::new(1024, 3, Duration::from_secs(1));
        reporter.observe(0);
        // 立即再次观察，应该返回 None（去抖）
        assert!(reporter.observe(512).is_none());
    }

    #[test]
    fn format_bytes_human_readable() {
        assert_eq!(ProgressReporter::format_bytes(0), "0.0 B");
        assert_eq!(ProgressReporter::format_bytes(1024), "1.0 KB");
        assert_eq!(ProgressReporter::format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(ProgressReporter::format_bytes(1024 * 1024 * 5), "5.0 MB");
    }
}