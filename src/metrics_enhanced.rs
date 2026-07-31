//! 增强版性能指标：滑动平均 + 趋势指示 + 历史峰值
//!
//! 重构说明：
//!   - 原有 metrics.rs 只返回瞬时采样值，容易抖动。
//!   - 本模块提供滑动平均窗口（5 次采样），趋势计算，以及峰值追踪。

use crate::server::metrics::Metrics;
use std::collections::VecDeque;

const WINDOW_SIZE: usize = 5;

/// 滑动平均窗口
pub struct MetricsSmoother {
    window: VecDeque<Metrics>,
    peak_cpu: f32,
    peak_vram_pct: f32,
    peak_gpu_pct: f32,
}

impl MetricsSmoother {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW_SIZE),
            peak_cpu: 0.0,
            peak_vram_pct: 0.0,
            peak_gpu_pct: 0.0,
        }
    }
    
    /// 添加新采样，返回平滑后的指标和趋势
    pub fn push(&mut self, metrics: Metrics) -> SmoothedMetrics {
        self.window.push_back(metrics.clone());
        if self.window.len() > WINDOW_SIZE {
            self.window.pop_front();
        }
        
        // 更新峰值
        if metrics.cpu_percent > self.peak_cpu {
            self.peak_cpu = metrics.cpu_percent;
        }
        let vram_pct = if metrics.gpu_mem_total_mb > 0.0 {
            (metrics.gpu_mem_used_mb / metrics.gpu_mem_total_mb) * 100.0
        } else {
            0.0
        };
        if vram_pct > self.peak_vram_pct {
            self.peak_vram_pct = vram_pct;
        }
        if metrics.gpu_util_pct > self.peak_gpu_pct {
            self.peak_gpu_pct = metrics.gpu_util_pct;
        }
        
        // 计算滑动平均
        let avg = self.compute_average();
        
        // 计算趋势
        let trend = self.compute_trend();
        
        SmoothedMetrics {
            metrics: avg,
            trend,
            peak_cpu: self.peak_cpu,
            peak_vram_pct: self.peak_vram_pct,
            peak_gpu_pct: self.peak_gpu_pct,
        }
    }
    
    fn compute_average(&self) -> Metrics {
        if self.window.is_empty() {
            return Metrics::default();
        }
        
        let sum_cpu: f32 = self.window.iter().map(|m| m.cpu_percent).sum();
        let sum_virt: u64 = self.window.iter().map(|m| m.virtual_size_bytes).sum();
        let sum_gpu_used: f32 = self.window.iter().map(|m| m.gpu_mem_used_mb).sum();
        let sum_gpu_total: f32 = self.window.iter().map(|m| m.gpu_mem_total_mb).sum();
        let sum_gpu_util: f32 = self.window.iter().map(|m| m.gpu_util_pct).sum();
        let sum_app_mem: u64 = self.window.iter().map(|m| m.app_memory_bytes).sum();
        
        let n = self.window.len() as f32;
        Metrics {
            cpu_percent: sum_cpu / n,
            virtual_size_bytes: (sum_virt as f32 / n) as u64,
            gpu_mem_used_mb: sum_gpu_used / n,
            gpu_mem_total_mb: sum_gpu_total / n,
            gpu_util_pct: sum_gpu_util / n,
            app_memory_bytes: (sum_app_mem as f32 / n) as u64,
            ..self.window.back().cloned().unwrap_or_default()
        }
    }
    
    fn compute_trend(&self) -> Trend {
        if self.window.len() < 2 {
            return Trend::Stable;
        }
        
        let recent = self.window.back().unwrap();
        let older = self.window.front().unwrap();
        
        let cpu_delta = recent.cpu_percent - older.cpu_percent;
        if cpu_delta.abs() < 2.0 {
            Trend::Stable
        } else if cpu_delta > 0.0 {
            Trend::Up
        } else {
            Trend::Down
        }
    }
    
    pub fn reset_peaks(&mut self) {
        self.peak_cpu = 0.0;
        self.peak_vram_pct = 0.0;
        self.peak_gpu_pct = 0.0;
    }
}

impl Default for MetricsSmoother {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
    Stable,
}

impl Trend {
    /// 转换为前端显示的箭头符号
    pub fn arrow(&self) -> &'static str {
        match self {
            Trend::Up => "↑",
            Trend::Down => "↓",
            Trend::Stable => "→",
        }
    }
    
    /// 转换为 CSS class 前缀
    pub fn class(&self) -> &'static str {
        match self {
            Trend::Up => "trend-up",
            Trend::Down => "trend-down",
            Trend::Stable => "trend-stable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SmoothedMetrics {
    pub metrics: Metrics,
    pub trend: Trend,
    pub peak_cpu: f32,
    pub peak_vram_pct: f32,
    pub peak_gpu_pct: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metrics(cpu: f32) -> Metrics {
        Metrics {
            pid: 1234,
            cpu_percent: cpu,
            virtual_size_bytes: 1000000,
            total_mem_bytes: 16000000000,
            uptime_secs: 100,
            port: 8080,
            gpu_mem_used_mb: 0.0,
            gpu_mem_total_mb: 0.0,
            gpu_util_pct: -1.0,
            app_memory_bytes: 50000000,
        }
    }

    #[test]
    fn smoother_empty_window() {
        let mut smoother = MetricsSmoother::new();
        let result = smoother.push(make_metrics(50.0));
        assert_eq!(result.metrics.cpu_percent, 50.0);
        assert_eq!(result.trend, Trend::Stable); // 只有一个采样时 trend 为 Stable
    }

    #[test]
    fn smoother_computes_average() {
        let mut smoother = MetricsSmoother::new();
        smoother.push(make_metrics(40.0));
        smoother.push(make_metrics(50.0));
        smoother.push(make_metrics(60.0));
        let result = smoother.push(make_metrics(70.0));
        assert_eq!(result.metrics.cpu_percent, 55.0); // (40+50+60+70)/4
    }

    #[test]
    fn smoother_window_size_limit() {
        let mut smoother = MetricsSmoother::new();
        for i in 0..10 {
            smoother.push(make_metrics(i as f32 * 10.0));
        }
        // 窗口应该只有最后 5 个
        assert_eq!(smoother.window.len(), WINDOW_SIZE);
    }

    #[test]
    fn trend_detection() {
        let mut smoother = MetricsSmoother::new();
        smoother.push(make_metrics(10.0));
        smoother.push(make_metrics(20.0));
        let result = smoother.push(make_metrics(30.0));
        assert_eq!(result.trend, Trend::Up);
        
        smoother.push(make_metrics(25.0));
        smoother.push(make_metrics(20.0));
        let result = smoother.push(make_metrics(15.0));
        assert_eq!(result.trend, Trend::Down);
    }

    #[test]
    fn peak_tracking() {
        let mut smoother = MetricsSmoother::new();
        smoother.push(make_metrics(30.0));
        smoother.push(make_metrics(50.0));
        smoother.push(make_metrics(40.0));
        assert_eq!(smoother.peak_cpu, 50.0);
        
        smoother.reset_peaks();
        assert_eq!(smoother.peak_cpu, 0.0);
    }

    #[test]
    fn trend_arrow() {
        assert_eq!(Trend::Up.arrow(), "↑");
        assert_eq!(Trend::Down.arrow(), "↓");
        assert_eq!(Trend::Stable.arrow(), "→");
    }

    #[test]
    fn trend_class() {
        assert_eq!(Trend::Up.class(), "trend-up");
        assert_eq!(Trend::Down.class(), "trend-down");
        assert_eq!(Trend::Stable.class(), "trend-stable");
    }
}
