//! 后台任务工厂：从子进程派生 5 个独立 tokio 任务。
//!
//! 设计动机（DEFECT-003 / DEFECT-006）：
//! - 拆分前 `lifecycle.rs` ~700 行，5 个 spawn 全堆在 `start()` 内部，
//!   屏蔽了「进程生命周期 vs 实时数据流」两条独立的关注线。
//! - 抽到本模块后 `lifecycle.rs::start` 只剩"派生 + 原子注册"两件事，
//!   阅读时一眼可看到 child + job + tasks 的组装关系。
//! - 每个任务工厂返回 `JoinHandle<()>`，由 `lifecycle::start` 收集到
//!   同一个 `Vec<JoinHandle<()>>` 中，并在 `stop` 时统一 abort。
//!
//! # 任务清单
//! - `spawn_stdout_reader` / `spawn_stderr_reader`：把子进程流写入
//!   有界 mpsc（[`super::log_channel::create`]），溢出计数见
//!   [`super::log_channel::DROPPED_LOG_LINES`]。
//! - `spawn_log_pump`：从 mpsc 抽行，emit 到前端 + 写入内存日志缓冲，
//!   容量超限则 drain 头部。同时按每 100 条 dropped 提醒一次。
//! - `spawn_watcher`：400ms 间隔 poll 子进程退出状态，触发状态机
//!   `Running → Stopped` / `Running → Crashed` 转移。
//! - `spawn_metrics_sampler`：100ms 采样、5 次平均后 500ms 推送一次
//!   `Metrics` 事件，含 CPU / 虚拟地址空间 / GPU 占用等。
//!
//! # 关键不变量
//! - 所有任务通过闭包捕获 `Arc<parking_lot::Mutex<ServerInner>>`，
//!   不要在闭包内持锁跨越 `.await`。
//! - 每个任务的退出条件：「`inner.pid` 不再等于自己启动时的 pid」
//!   或「子进程被 stop / watcher 已设置 status」。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use sysinfo::Pid;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::task::JoinHandle;

use crate::events::{LogLine, ServerStatus};
use crate::log::emit_log;
use crate::util::time::now_ts;

use super::log_channel::{try_send_or_count, DROPPED_LOG_LINES, LOG_CHANNEL_CAPACITY};
use super::log_truncate::truncate_log_line;
use super::metrics::{query_gpu_stats, Metrics, METRICS_INTERVAL_MS};
use super::state::{ServerInner, MAX_LOG_LINES};
use super::winapi::query_windows_virtual_size;

use sysinfo::{ProcessesToUpdate, System};

/// 子进程 stdout → mpsc 任务。
///
/// 每行包装为 `LogLine { stream: "stdout" }`，送入共享通道。
/// mpsc 满时 `try_send_or_count` 自动丢弃 + 累加 [`DROPPED_LOG_LINES`]。
pub(crate) fn spawn_stdout_reader(
    stdout: ChildStdout,
    tx: tokio::sync::mpsc::Sender<LogLine>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let entry = LogLine {
                timestamp: now_ts(),
                stream: "stdout".into(),
                text: truncate_log_line(&line),
                group: None,
            };
            let _ = try_send_or_count(&tx, entry);
        }
    })
}

/// 子进程 stderr → mpsc 任务。
pub(crate) fn spawn_stderr_reader(
    stderr: ChildStderr,
    tx: tokio::sync::mpsc::Sender<LogLine>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let entry = LogLine {
                timestamp: now_ts(),
                stream: "stderr".into(),
                text: truncate_log_line(&line),
                group: None,
            };
            let _ = try_send_or_count(&tx, entry);
        }
    })
}

/// mpsc → 前端 + 内存缓冲任务。
///
/// 优化点（DEFECT-006 节流）：
/// - 先 `emit("server-log", &line)`（借用），再 `push(line)`（move），
///   避免 1 次 `String` clone（burst 1000+ 行/s 时差异显著）。
/// - 缓冲超 `MAX_LOG_LINES` 时 `drain(0..drop_n)` 头部。
/// - 每累计 100 条 dropped 直发一条系统告警（不走 channel，
///   避免"系统日志也被丢弃"的鸡生蛋）。
pub(crate) fn spawn_log_pump(
    mut rx: tokio::sync::mpsc::Receiver<LogLine>,
    app: AppHandle,
    inner: Arc<parking_lot::Mutex<ServerInner>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_emitted_dropped: u64 = 0;
        while let Some(line) = rx.recv().await {
            // 1) emit 走 &line 借用（serde 序列化一次）
            let _ = app.emit("server-log", &line);
            // 2) move line 进 Vec
            {
                let mut guard = inner.lock();
                guard.logs.push(line);
                if guard.logs.len() > MAX_LOG_LINES {
                    let drop_n = guard.logs.len() - MAX_LOG_LINES;
                    guard.logs.drain(0..drop_n);
                }
            }

            // 3) 节流告警：每累计 100 条 dropped 提醒一次
            let dropped = DROPPED_LOG_LINES.load(Ordering::Relaxed);
            if dropped >= last_emitted_dropped + 100 {
                last_emitted_dropped = (dropped / 100) * 100;
                let sys_line = LogLine {
                    timestamp: now_ts(),
                    stream: "system".into(),
                    text: format!(
                        "[警告] 已丢弃 {} 条日志（通道容量 {}），前端可能繁忙",
                        last_emitted_dropped, LOG_CHANNEL_CAPACITY
                    ),
                    group: None,
                };
                let _ = app.emit("server-log", &sys_line);
                let mut guard = inner.lock();
                guard.logs.push(sys_line);
                if guard.logs.len() > MAX_LOG_LINES {
                    let drop_n = guard.logs.len() - MAX_LOG_LINES;
                    guard.logs.drain(0..drop_n);
                }
            }
        }
    })
}

/// 子进程退出 watcher：400ms 间隔 poll，状态机转移。
///
/// 设计：使用 `c.try_wait()` 非阻塞检测；进程已退出时设置
/// `ServerStatus::Stopped` / `Crashed` 并 emit。
pub(crate) fn spawn_watcher(
    inner: Arc<parking_lot::Mutex<ServerInner>>,
    app: AppHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;

            let exit: Option<std::process::ExitStatus> = {
                let mut guard = inner.lock();
                match guard.child.as_mut() {
                    Some(c) => c.try_wait().ok().flatten(),
                    None => None,
                }
            };

            if let Some(status) = exit {
                let mut guard = inner.lock();
                guard.child = None;
                guard.pid = None;
                guard.started_at = None;
                guard.active_port = None;
                if status.success() {
                    guard.status = ServerStatus::Stopped;
                    drop(guard);
                    crate::log::emit_status(&app, ServerStatus::Stopped);
                    emit_log(&app, "system", "llama-server 已正常退出。");
                } else {
                    guard.status = ServerStatus::Crashed;
                    drop(guard);
                    crate::log::emit_status(&app, ServerStatus::Crashed);
                    emit_log(
                        &app,
                        "system",
                        &format!("llama-server 异常退出，状态码：{}", status),
                    );
                }
                break;
            }

            let still_present = inner.lock().child.is_some();
            if !still_present {
                break;
            }
        }
    })
}

/// 指标采样器：100ms 采样，5 次平均后 500ms emit `server-metrics` 事件。
///
/// 指标组成：
/// - `cpu_percent`：sysinfo `Process::cpu_usage()`（基于内核 jiffies）
/// - `virtual_size_bytes`：Windows 走 [`query_windows_virtual_size`]
///   （即 `NtQueryInformationProcess + ProcessVmCounters.VirtualSize`），
///   包含 mmap 映射文件，更准确反映 llama.cpp mmap 的 GGUF 大小；
///   其他平台退回到 sysinfo `Process::virtual_memory()`
/// - `app_memory_bytes`：本应用（`llama-ui.exe`）自身的 `p.memory()`
/// - `gpu_*`：来自 [`query_gpu_stats`]（内部 5s 缓存）
///
/// 退出条件：`inner.pid` 不再等于自己启动时的 pid（即 stop 之后被覆盖）。
pub(crate) fn spawn_metrics_sampler(
    inner: Arc<parking_lot::Mutex<ServerInner>>,
    app: AppHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sys = System::new();
        // 累积器：每 5 次采样归零
        let mut acc_cpu = 0.0f32;
        let mut acc_virt = 0u64;
        let mut acc_total_mem = 0u64;
        let mut acc_gpu_used = 0.0f32;
        let mut acc_gpu_total = 0.0f32;
        let mut acc_gpu_util = 0.0f32;
        let mut acc_app_memory = 0u64;
        let mut sample_count = 0u32;

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(METRICS_INTERVAL_MS)).await;

            // Snapshot what we need with the lock held briefly.
            let (pid_opt, uptime, port) = {
                let guard = inner.lock();
                let uptime = guard.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                (guard.pid, uptime, guard.active_port)
            };

            let pid = match pid_opt {
                Some(p) => p,
                None => break, // process gone
            };
            let port = match port {
                Some(p) => p,
                None => break,
            };

            // ---- 计算进程内存指标 ----
            // 总虚拟地址空间（virtual_size_bytes）：
            //   包含 mmap 映射文件（= `VM_COUNTERS.VirtualSize`）。
            //   适合用来观察 llama.cpp mmap 了多大的 GGUF 文件。
            //
            // sysinfo 0.30+ 中 memory()/total_memory() 均直接返回 bytes。
            sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]));
            sys.refresh_memory();
            let total_mem = sys.total_memory();

            // 从 sysinfo 拿 CPU / 虚拟地址空间
            // - virtual_memory() → VirtualSize（虚拟地址空间，bytes），对应任务管理器「虚拟大小」列
            let (cpu, virt_sysinfo) = sys
                .process(Pid::from_u32(pid))
                .map(|p| (p.cpu_usage(), p.virtual_memory()))
                .unwrap_or((0.0, 0));

            // Windows：覆盖 virtual_size_bytes（更准确）
            // 失败兜底为 sysinfo 的值。
            #[cfg(windows)]
            let virtual_size_bytes = {
                let virt = query_windows_virtual_size(pid);
                if virt > 0 {
                    virt
                } else {
                    virt_sysinfo
                }
            };
            #[cfg(not(windows))]
            let virtual_size_bytes = virt_sysinfo;

            // Query GPU stats (异步、不阻塞 metrics 主循环)，内部已带 5s 缓存
            let (gpu_used, gpu_total, gpu_util) = query_gpu_stats().await;

            // ---- 获取本应用物理内存用量 ----
            // 用 sysinfo 查找当前进程（llama-ui.exe）的内存占用
            let app_pid = std::process::id();
            let app_memory = sys
                .process(Pid::from_u32(app_pid))
                .map(|p| p.memory())
                .unwrap_or(0);

            // ---- 累积 ----
            acc_cpu += cpu;
            acc_virt += virtual_size_bytes;
            acc_total_mem += total_mem;
            acc_gpu_used += gpu_used;
            acc_gpu_total += gpu_total;
            acc_gpu_util += gpu_util;
            acc_app_memory += app_memory;
            sample_count += 1;

            // ---- 每 5 次采样（500ms）发射一次平均值 ----
            if sample_count >= 5 {
                let count = sample_count as f32;
                let m = Metrics {
                    pid,
                    cpu_percent: acc_cpu / count,
                    virtual_size_bytes: (acc_virt as f64 / f64::from(sample_count)) as u64,
                    total_mem_bytes: (acc_total_mem as f64 / f64::from(sample_count)) as u64,
                    uptime_secs: uptime,
                    port,
                    gpu_mem_used_mb: acc_gpu_used / count,
                    gpu_mem_total_mb: acc_gpu_total / count,
                    gpu_util_pct: acc_gpu_util / count,
                    app_memory_bytes: (acc_app_memory as f64 / f64::from(sample_count)) as u64,
                };
                let _ = app.emit("server-metrics", &m);

                // 重置累积器
                acc_cpu = 0.0;
                acc_virt = 0;
                acc_total_mem = 0;
                acc_gpu_used = 0.0;
                acc_gpu_total = 0.0;
                acc_gpu_util = 0.0;
                acc_app_memory = 0;
                sample_count = 0;
            }

            // Bail if the process is no longer the active one.
            let still_active = inner.lock().pid == Some(pid);
            if !still_active {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    //! 任务工厂无单测（每个都涉及 tokio runtime + AppHandle 真实环境）；
    //! 集成测试在 [`crate::commands::server_cmd`] 中通过 stop 路径验证
    //! 任务能被正确 abort。
    use super::*;

    /// 编译期断言：所有任务工厂都返回 `JoinHandle<()>`，
    /// 防止后续重构把签名改坏（`lifecycle::start` 收集到同一个 Vec）。
    #[allow(dead_code)]
    fn _signature_invariants() {
        fn _check_stdout(s: ChildStdout, tx: tokio::sync::mpsc::Sender<LogLine>) -> JoinHandle<()> {
            spawn_stdout_reader(s, tx)
        }
        fn _check_stderr(s: ChildStderr, tx: tokio::sync::mpsc::Sender<LogLine>) -> JoinHandle<()> {
            spawn_stderr_reader(s, tx)
        }
    }
}
