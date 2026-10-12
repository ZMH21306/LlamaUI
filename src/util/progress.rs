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
    // 平滑参数（可配置）
    speed_alpha: f64,              // EMA 平滑系数，范围 (0, 1)，默认 0.2
    eta_decrease_limit: f64,       // 每次更新 ETA 最多下降当前值的比例，默认 0.5
    // 平滑状态
    smoothed_speed_bps: f64,       // EMA 平滑后的瞬时速度 (bytes/s)
    smoothed_eta_secs: Option<u64>,
    smooth_samples: usize,         // 已累计采样次数（用于 warm-up）
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
            speed_alpha: 0.2,
            eta_decrease_limit: 0.5,
            smoothed_speed_bps: 0.0,
            smoothed_eta_secs: None,
            smooth_samples: 0,
        }
    }

    /// 设置 EMA 平滑系数（0 < alpha <= 1）。
    /// alpha 越小越平滑（响应越慢）。
    pub fn with_speed_alpha(mut self, alpha: f64) -> Self {
        self.speed_alpha = alpha.clamp(0.001, 1.0);
        self
    }

    /// 设置 ETA 下降限制（0 <= limit <= 1）。
    /// limit 越小越保守（ETA 下降越慢）。
    /// 例如 limit=0.5 表示每次更新 ETA 最多下降当前值的 50%。
    pub fn with_eta_decrease_limit(mut self, limit: f64) -> Self {
        self.eta_decrease_limit = limit.clamp(0.0, 1.0);
        self
    }

    /// 记录一个 chunk 的到达。每次调用会更新内部平滑状态，
    /// 并在达到 UI 更新间隔时返回平滑后的进度。
    ///
    /// 返回 `(progress, downloaded, smoothed_speed_bps, smoothed_eta_secs)`；
    /// 如果未达到 UI 更新间隔，则返回 `None`。
    pub fn observe(&mut self, downloaded: u64) -> Option<(f64, u64, f64, Option<u64>)> {
        let now = Instant::now();
        // 添加新样本到滑动窗口
        self.samples.push(Sample {
            downloaded,
            instant: now,
        });
        if self.samples.len() > self.window_size {
            self.samples.remove(0);
        }

        // 计算瞬时速度和瞬时 ETA（基于滑动窗口）
        let (progress, _, instant_speed_bps, _instant_eta) = self.compute(downloaded);

        // EMA 平滑瞬时速度
        if self.smooth_samples == 0 {
            self.smoothed_speed_bps = instant_speed_bps;
        } else {
            self.smoothed_speed_bps =
                self.speed_alpha * instant_speed_bps + (1.0 - self.speed_alpha) * self.smoothed_speed_bps;
        }
        self.smooth_samples += 1;

        // 基于平滑速度计算 ETA 并施加下降限制
        let smoothed_eta_option = if self.smoothed_speed_bps > 0.0 && self.total > downloaded {
            let mut eta = ((self.total - downloaded) as f64 / self.smoothed_speed_bps) as u64;
            if let Some(last_eta) = self.smoothed_eta_secs {
                // 限制 ETA 下降速度：每次更新最多下降当前值的 (1 - eta_decrease_limit)
                // 例如 eta_decrease_limit=0.5 时，每次最多下降 50%
                let min_allowed = ((last_eta as f64) * self.eta_decrease_limit).ceil().max(1.0) as u64;
                if eta < last_eta && eta < min_allowed {
                    eta = min_allowed;
                }
            }
            Some(eta)
        } else {
            None
        };
        self.smoothed_eta_secs = smoothed_eta_option;

        // 检查是否达到 UI 更新间隔
        if now.duration_since(self.last_emit) < self.min_interval {
            return None;
        }
        self.last_emit = now;

        // 返回平滑后的进度（速度转换为 MB/s 由调用方处理，这里保持 bps）
        Some((
            progress,
            downloaded,
            self.smoothed_speed_bps,
            self.smoothed_eta_secs,
        ))
    }

    /// 强制发射进度（即使没有新 chunk 到达）。
    /// 用于定时器回调，确保 UI 定期刷新而不是卡在上一次 emit。
    ///
    /// 返回 `(progress, downloaded, smoothed_speed_bps, smoothed_eta_secs)`，
    /// 其中速度和 ETA 为上次 observe 的平滑结果（若尚未有样本则为零）。
    pub fn force_emit(&mut self, downloaded: u64) -> Option<(f64, u64, f64, Option<u64>)> {
        let now = Instant::now();
        if now.duration_since(self.last_emit) < self.min_interval {
            return None;
        }
        self.last_emit = now;

        let progress = if self.total > 0 {
            downloaded as f64 / self.total as f64
        } else {
            0.0
        };
        Some((
            progress,
            downloaded,
            self.smoothed_speed_bps,
            self.smoothed_eta_secs,
        ))
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

    #[test]
    fn progress_reporter_unknown_total() {
        let mut reporter = ProgressReporter::new(0, 3, Duration::from_millis(0));
        reporter.observe(0);
        thread::sleep(Duration::from_millis(10));
        reporter.observe(1024);
        let (progress, downloaded, _, eta) = reporter.observe(2048).unwrap();
        assert_eq!(progress, 0.0);
        assert_eq!(downloaded, 2048);
        assert!(eta.is_none());
    }

    #[test]
    fn progress_reporter_eta_calculation() {
        let total = 10000;
        let mut reporter = ProgressReporter::new(total, 3, Duration::from_millis(0));
        reporter.observe(0);
        thread::sleep(Duration::from_millis(50));
        reporter.observe(0);
        let (_, downloaded, _, _eta) = reporter.observe(total / 2).unwrap();
        assert_eq!(downloaded, total / 2);
        // With total=10000 and enough samples, ETA should be calculated
        // This test mainly verifies the function runs without panic
    }
}
