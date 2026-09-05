// GPU 指标采样 + 缓存
// 避免每 1.5s 启动一次 nvidia-smi，每 GPU_CACHE_TTL 秒最多查询一次。

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::util::process::silent_tokio_command;

/// 默认缓存时间（秒）。nvidia-smi 的数据 1s 内几乎不变，没必要每次 metrics 间隔
/// （1.5s）都查询。
const GPU_CACHE_TTL: Duration = Duration::from_secs(5);

/// 底层指标采样间隔（毫秒）。100ms = 0.1s，
/// 5 次采样取平均后向上 emit 一次（即 500ms 更新一次 UI）。
pub const METRICS_INTERVAL_MS: u64 = 100;

/// GPU 数据快照，附带采样时间。
struct CachedGpu {
    used_mb: f32,
    total_mb: f32,
    util_pct: f32,
    fetched_at: Instant,
}

/// 全局 GPU 缓存。`Mutex<Option<...>>` 以便第一次调用时填充。
static GPU_CACHE: Mutex<Option<CachedGpu>> = Mutex::new(None);

/// 周期性的进程指标快照（发给前端）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub pid: u32,
    pub cpu_percent: f32,
    /// 总虚拟地址空间（bytes）。含 mmap 映射文件、reserved-but-uncommitted 区域。
    /// Windows 任务管理器"虚拟大小"列（默认不显示，需手动勾选）。
    /// **对 llama.cpp 加载的 GGUF 模型**：mmap 后的 GGUF 大小会显示在这里
    /// （例如 4GB 模型 → ~4GB）。
    pub virtual_size_bytes: u64,
    /// 系统总物理内存 (bytes)
    pub total_mem_bytes: u64,
    pub uptime_secs: u64,
    pub port: u16,
    /// GPU 显存已用 (MiB)；0 表示不可用（无 NVIDIA 驱动 / 无 nvidia-smi）。
    pub gpu_mem_used_mb: f32,
    /// GPU 显存总量 (MiB)；0 表示不可用。
    pub gpu_mem_total_mb: f32,
    /// GPU 利用率 (0-100)；-1 表示不可用。
    pub gpu_util_pct: f32,
    /// 本应用物理内存用量 (bytes)。监控自身内存占用。
    pub app_memory_bytes: u64,
}

/// 通过 nvidia-smi 查询 GPU 0 的 (显存已用 MiB, 显存总量 MiB, 利用率 %)。
/// 内部带 5s 缓存，避免每 1.5s fork 一次进程。不可用时返回 (0, 0, -1)。
pub async fn query_gpu_stats() -> (f32, f32, f32) {
    // 命中缓存
    {
        if let Ok(g) = GPU_CACHE.lock() {
            if let Some(c) = g.as_ref() {
                if c.fetched_at.elapsed() < GPU_CACHE_TTL {
                    return (c.used_mb, c.total_mb, c.util_pct);
                }
            }
        }
    }
    // 真正查询
    let (used, total, util) = query_gpu_stats_uncached().await;
    if let Ok(mut g) = GPU_CACHE.lock() {
        *g = Some(CachedGpu {
            used_mb: used,
            total_mb: total,
            util_pct: util,
            fetched_at: Instant::now(),
        });
    }
    (used, total, util)
}

/// 不带缓存的 GPU 查询（测试与内部使用）。
async fn query_gpu_stats_uncached() -> (f32, f32, f32) {
    // 跨平台调用：Windows / Linux / macOS 上 NVIDIA 驱动都自带 nvidia-smi
    let mut cmd = silent_tokio_command("nvidia-smi");
    cmd.arg("--query-gpu=memory.used,memory.total,utilization.gpu")
        .arg("--format=csv,noheader,nounits");

    let out = cmd.output().await;
    let out = match out {
        Ok(o) if o.status.success() => o,
        _ => return (0.0, 0.0, -1.0),
    };
    let s = String::from_utf8_lossy(&out.stdout);
    // 取首行（多卡时只看 GPU 0 这一行）
    let line = match s.lines().next() {
        Some(l) => l,
        None => return (0.0, 0.0, -1.0),
    };
    let parts: Vec<&str> = line.split(',').map(|x| x.trim()).collect();
    if parts.len() < 3 {
        return (0.0, 0.0, -1.0);
    }
    let used: f32 = parts[0].parse().unwrap_or(0.0);
    let total: f32 = parts[1].parse().unwrap_or(0.0);
    let util: f32 = parts[2].parse().unwrap_or(-1.0);
    (used, total, util)
}
