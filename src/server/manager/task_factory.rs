// Fixed version with proper cancellation support
//!
//! 本模块提取并扩展自 `src/server/tasks.rs`，通过工厂模式实现
//! 所有后台任务的统一管理和创建。
//! 
//! 设计目标：
//! 1. 任务抽象：定义 `BackgroundTask` trait，实现任务的统一管理
//! 2. 任务工厂：`TaskManager` 负责所有任务的生命周期管理
//! 3. 状态解耦：任务通过状态存储获取状态，不直接访问原始内部结构
//! 4. 扩展性：支持动态添加新任务类型
//!
//! 任务类型：
//! - 标准输出读取器 (`StdoutReaderTask`)
//! - 错误输出读取器 (`StderrReaderTask`) 
//! - 日志泵 (`LogPumpTask`)
//! - 状态监视器 (`WatcherTask`)
//! - 指标采样器 (`MetricsSamplerTask`)
use crate::constants::*;
use crate::events::{LogLine, ServerStatus, EVT_SERVER_LOG, EVT_SERVER_METRICS};
use crate::log::emit_log;
use crate::server::manager::state_store::ServerStateStore;
use crate::server::metrics::Metrics;
use crate::util::time::now_ts;
use std::sync::Arc;
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinHandle;
/// 后台任务抽象 - 所有后台任务必须实现的接口
///
/// 任务通过工厂管理，统一管理生命周期和状态访问
pub trait BackgroundTask: Send + Sync {
    /// 获取任务名称，用于日志记录和调试
    fn name(&self) -> &'static str;
    
    /// 派生任务，获取状态存储引用
    /// 任务应该通过状态存储访问状态，而不是直接访问内部结构
    fn spawn(&mut self, state_store: Arc<ServerStateStore>) -> JoinHandle<()>;
}

/// 子进程标准输出读取器任务
///
/// 将子进程标准输出行写入共享日志通道
/// mpsc 通道满时自动丢弃，使用 `try_send_or_count` 实现非阻塞写入
pub struct StdoutReaderTask;

impl BackgroundTask for StdoutReaderTask {
    fn name(&self) -> &'static str {
        "stdout_reader"
    }
    
    fn spawn(&mut self, state_store: Arc<ServerStateStore>) -> JoinHandle<()> {
        // 从子进程获取stdout，并将log_tx取出
        // 使用 take() 移除 log_tx，因为我们需要将它移动到 async 闭包中
        let (stdout, tx) = {
            let mut inner = state_store.get_mut();
            let stdout = match &mut inner.child {
                Some(child) => match child.stdout.take() {
                    Some(stdout) => stdout,
                    None => return tokio::spawn(async {}),
                },
                None => return tokio::spawn(async {}),
            };
            let tx = inner.log_tx.take().expect("Log channel should be initialized");
            (stdout, tx)
        };
        
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(text)) => {
                        let entry = LogLine {
                            timestamp: now_ts(),
                            stream: "stdout".into(),
                            text: text.clone(),
                            group: None,
                        };
                        let _ = tx.try_send(entry);
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        })
    }
}

/// 子进程错误输出读取器任务
///
/// 将子进程错误输出行写入共享日志通道
/// 错误输出通常包含诊断信息，对于日志记录非常重要
pub struct StderrReaderTask;

impl BackgroundTask for StderrReaderTask {
    fn name(&self) -> &'static str {
        "stderr_reader"
    }
    
    fn spawn(&mut self, state_store: Arc<ServerStateStore>) -> JoinHandle<()> {
        // 从子进程获取stderr，并将log_tx取出
        let (stderr, tx) = {
            let mut inner = state_store.get_mut();
            let stderr = match &mut inner.child {
                Some(child) => match child.stderr.take() {
                    Some(stderr) => stderr,
                    None => return tokio::spawn(async {}),
                },
                None => return tokio::spawn(async {}),
            };
            let tx = inner.log_tx.take().expect("Log channel should be initialized");
            (stderr, tx)
        };
        
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(text)) => {
                        let entry = LogLine {
                            timestamp: now_ts(),
                            stream: "stderr".into(),
                            text: text.clone(),
                            group: None,
                        };
                        let _ = tx.try_send(entry);
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        })
    }
}

/// 日志泵任务 - 从共享通道提取日志并发送给前端
///
/// 负责创建日志通道、协调日志存储和前端事件，保证日志的一致性和完整性
pub struct LogPumpTask;

impl BackgroundTask for LogPumpTask {
    fn name(&self) -> &'static str {
        "log_pump"
    }
    
    fn spawn(&mut self, state_store: Arc<ServerStateStore>) -> JoinHandle<()> {
        // 创建日志通道
        let (tx, mut rx) = tokio::sync::mpsc::channel(crate::server::log_channel::LOG_CHANNEL_CAPACITY);
        
        // 存储发送端到状态存储，供 stdout/stderr reader 使用
        {
            let mut inner = state_store.get_mut();
            inner.log_tx = Some(tx);
        }
        
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_line = rx.recv() => {
                        match maybe_line {
                            Some(log_line) => {
                                // 将日志发送到前端
                                let inner_ref = state_store.get();
                                if let Some(app) = &inner_ref.app_handle {
                                    let _ = app.emit(&EVT_SERVER_LOG, &log_line);
                                }
                                drop(inner_ref);
                                // 同时写入内存缓冲
                                let mut inner = state_store.get_mut();
                                inner.logs.push(log_line);
                                // 控制缓冲区大小
                                let current_len = inner.logs.len();
                                if current_len > MAX_LOG_LINES {
                                    let keep = MAX_LOG_LINES - 100;
                                    inner.logs.drain(0..current_len - keep);
                                }
                            }
                            None => {
                                // 通道已关闭，退出任务
                                break;
                            }
                        }
                    },
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
                }
            }
        })
    }
}

/// 状态监视器任务 - 监控子进程状态变化并触发状态转换
///
/// 定期检查子进程状态，负责将 `Running` 状态转换为 `Stopped` 或 `Crashed`
pub struct WatcherTask;

impl BackgroundTask for WatcherTask {
    fn name(&self) -> &'static str {
        "watcher"
    }
    
    fn spawn(&mut self, state_store: Arc<ServerStateStore>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                // 检查子进程状态
                let pid = {
                    let inner = state_store.get();
                    inner.pid
                };
                
                if let Some(pid) = pid {
                    // 检查子进程是否还在运行
                    let still_running = {
                        let inner = state_store.get();
                        matches!(inner.status, ServerStatus::Running | ServerStatus::Starting)
                    };
                    
                    if !still_running {
                        // 进程已停止，更新状态
                        state_store.update_status(ServerStatus::Stopped);
                        if let Some(app) = &state_store.get().app_handle {
                            emit_log(app, "system", "llama-server 已停止");
                        }
                        break;
                    }
                    
                    // 检查进程是否退出（使用 try_wait）
                    #[cfg(windows)]
                    {
                        use std::process::Command;
                        let output = Command::new("tasklist")
                            .arg("/FI")
                            .arg(format!("PID eq {}", pid))
                            .output();
                        
                        if let Ok(output) = output {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            if !stdout.contains(&pid.to_string()) {
                                // 进程已退出，更新状态
                                state_store.update_status(ServerStatus::Stopped);
                                if let Some(app) = &state_store.get().app_handle {
                                    emit_log(app, "system", "llama-server 已停止");
                                }
                                break;
                            }
                        }
                    }
                } else {
                    // 没有活跃的 PID，检查状态
                    let status = state_store.status();
                    if matches!(status, ServerStatus::Running | ServerStatus::Starting) {
                        // 状态显示运行但没有 PID，可能是异常状态
                        state_store.update_status(ServerStatus::Stopped);
                    }
                }
                
                tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
            }
        })
    }
}

/// 指标采样任务 - 定期采样服务器指标并发送给前端
///
/// 采样包括CPU使用率、内存使用、GPU指标等，每5次采样后推送一次
pub struct MetricsSamplerTask;

impl BackgroundTask for MetricsSamplerTask {
    fn name(&self) -> &'static str {
        "metrics_sampler"
    }
    
    fn spawn(&mut self, state_store: Arc<ServerStateStore>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut acc_cpu = 0.0;
            let mut acc_virt = 0u64;
            let mut acc_total_mem = 0u64;
            let mut sample_count = 0u32;
            let mut last_pid: Option<u32> = None;
            
            let mut sys = sysinfo::System::new_all();
            
            loop {
                // 获取当前状态
                let (pid, port, uptime_secs) = {
                    let inner = state_store.get();
                    (inner.pid, inner.active_port, inner.started_at.map(|t| t.elapsed().as_secs()))
                };
                
                if let Some(pid) = pid {
                    // 采样 CPU 和内存
                    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]));
                    sys.refresh_memory();
                    
                    if let Some(process) = sys.process(sysinfo::Pid::from_u32(pid)) {
                        let cpu = process.cpu_usage();
                        let virtual_size = process.virtual_memory() as u64;
                        let total_mem = sys.total_memory();
                        
                        acc_cpu += cpu;
                        acc_virt += virtual_size;
                        acc_total_mem += total_mem;
                        sample_count += 1;
                        
                        // 每 5 次采样（约 500ms）发送一次
                        if sample_count >= 5 {
                            let m = Metrics {
                                pid,
                                cpu_percent: acc_cpu / sample_count as f32,
                                virtual_size_bytes: acc_virt / sample_count as u64,
                                total_mem_bytes: acc_total_mem / sample_count as u64,
                                uptime_secs: uptime_secs.unwrap_or(0),
                                port: port.unwrap_or(0),
                                gpu_mem_used_mb: 0.0,
                                gpu_mem_total_mb: 0.0,
                                gpu_util_pct: -1.0,
                                app_memory_bytes: 0,
                            };
                            
                            if let Some(app) = &state_store.get().app_handle {
                                let _ = app.emit(EVT_SERVER_METRICS, &m);
                            }
                            
                            acc_cpu = 0.0;
                            acc_virt = 0;
                            acc_total_mem = 0;
                            sample_count = 0;
                        }
                        
                        last_pid = Some(pid);
                    } else if last_pid.is_some() {
                        // 进程已退出
                        state_store.update_status(ServerStatus::Stopped);
                        break;
                    }
                } else {
                    // 没有 PID，等待
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                
                tokio::time::sleep(tokio::time::Duration::from_millis(METRICS_INTERVAL_MS)).await;
            }
        })
    }
}

/// 后台任务管理器 - 管理所有后台任务的生命周期
///
/// 通过工厂模式统一管理所有后台任务，包括启动、停止和状态检查
pub struct TaskManager {
    tasks: Vec<Box<dyn BackgroundTask>>,
    state_store: Arc<ServerStateStore>,
}

impl TaskManager {
    /// 创建新的任务管理器
    ///
    /// 会自动注册所有标准任务类型：
    /// - StdoutReaderTask
    /// - StderrReaderTask
    /// - LogPumpTask
    /// - WatcherTask
    /// - MetricsSamplerTask
    pub fn new(state_store: Arc<ServerStateStore>) -> Self {
        let tasks: Vec<Box<dyn BackgroundTask>> = vec![
            Box::new(StdoutReaderTask),
            Box::new(StderrReaderTask),
            Box::new(LogPumpTask),
            Box::new(WatcherTask),
            Box::new(MetricsSamplerTask),
        ];
        
        Self {
            tasks,
            state_store,
        }
    }
    
    /// 启动所有任务
    ///
    /// 遍历所有任务，调用其 spawn 方法派生任务
    /// 所有派生的任务会被添加到状态存储的任务列表中
    pub async fn start_all(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("start_all: 开始启动任务");
        for task in &mut self.tasks {
            println!("start_all: 正在启动任务: {}", task.name());
            let handle = task.spawn(self.state_store.clone());
            println!("start_all: 任务已派生");
            let mut inner = self.state_store.get_mut();
            inner.tasks.push(handle);
            println!("start_all: 任务已添加到状态存储，当前任务数: {}", inner.tasks.len());
        }
        println!("start_all: 所有任务启动完成");

    println!("start_all: 所有任务启动完成");
        Ok(())
    }
    
    /// 停止所有任务
    ///
    /// 取消所有任务的执行，清空任务列表
    /// 这与生命周期管理的 stop() 方法配合使用
    pub async fn stop_all(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("stop_all: 开始停止任务");
        let mut inner = self.state_store.get_mut();
        println!("stop_all: 获取锁，共 {} 个任务", inner.tasks.len());
        for handle in inner.tasks.iter_mut() {
            println!("stop_all: 停止任务");
            handle.abort();
            println!("stop_all: 任务中止成功");
        }
        inner.tasks.clear();
        println!("stop_all: 任务列表已清空");
        Ok(())
    }
    
    /// 获取所有任务的名称
    ///
    /// 用于调试和日志记录
    pub fn get_task_names(&self) -> Vec<&'static str> {
        self.tasks.iter().map(|task| task.name()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{LogLine, ServerStatus};
    use std::sync::Arc;

    #[test]
    fn new_creates_empty_inner_state() {
        let store = ServerStateStore::new();
        let inner = store.get();
        assert!(inner.tasks.is_empty(), "新存储的 tasks 必须为空");
        assert!(inner.child.is_none(), "新存储的 child 必须为 None");
        assert!(inner.pid.is_none(), "新存储的 pid 必须为 None");
        assert!(inner.started_at.is_none(), "新存储的 started_at 必须为 None");
        assert!(inner.active_port.is_none(), "新存储的 active_port 必须为 None");
        assert!(inner.job.is_none(), "新存储的 job 必须为 None");
        assert_eq!(inner.status, ServerStatus::Stopped, "新存储的 status 必须为 Stopped");
        assert!(inner.logs.is_empty(), "新存储的 logs 必须为空");
    }

    #[test]
    fn initial_status_is_stopped() {
        let store = ServerStateStore::new();
        assert_eq!(store.status(), ServerStatus::Stopped);
    }

    #[test]
    fn initial_active_port_is_none() {
        let store = ServerStateStore::new();
        assert_eq!(store.active_port(), None);
    }

    #[test]
    fn clear_logs_on_empty_is_noop() {
        let store = ServerStateStore::new();
        store.clear_logs();
        assert!(store.logs_snapshot().is_empty());
    }

    #[test]
    fn logs_snapshot_is_cloned() {
        let store = ServerStateStore::new();
        {
            let mut inner = store.get_mut();
            inner.logs.push(LogLine {
                timestamp: "2026-01-01 00:00:00".into(),
                stream: "stdout".into(),
                text: "hello".into(),
                group: None,
            });
        }
        let snap = store.logs_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].text, "hello");
        // 原始缓冲不受影响
        assert_eq!(store.logs_snapshot().len(), 1);
    }

    #[test]
    fn update_status_works() {
        let store = ServerStateStore::new();
        store.update_status(ServerStatus::Starting);
        assert_eq!(store.status(), ServerStatus::Starting);
    }

    #[test]
    fn is_status_works() {
        let store = ServerStateStore::new();
        assert!(store.is_status(ServerStatus::Stopped));
        store.update_status(ServerStatus::Running);
        assert!(!store.is_status(ServerStatus::Stopped));
        assert!(store.is_status(ServerStatus::Running));
    }

    #[test]
    fn task_names_are_correct() {
        let state_store = Arc::new(ServerStateStore::new());
        let manager = TaskManager::new(state_store);
        let task_names = manager.get_task_names();
        let expected_names = vec![
            "stdout_reader",
            "stderr_reader", 
            "log_pump",
            "watcher",
            "metrics_sampler"
        ];
        assert_eq!(task_names, expected_names);
    }

    #[tokio::test]
    async fn task_manager_start_stop_tasks() {
        use std::time::Duration;
        use tokio::time::sleep;
        
        let state_store = Arc::new(ServerStateStore::new());
        let mut manager = TaskManager::new(state_store);
        
        // 启动所有任务
        let start_result = manager.start_all().await;
        assert!(start_result.is_ok(), "启动任务失败: {:?}", start_result);
        
        // 验证任务已在状态存储中（验证 start_all 完成即可，不依赖任务执行状态）
        let inner = manager.state_store.get();
        assert!(!inner.tasks.is_empty(), "启动任务后应该有任务在运行");
        
        // 停止所有任务
        let stop_result = manager.stop_all().await;
        assert!(stop_result.is_ok(), "停止任务失败: {:?}", stop_result);
        
        // 验证任务已被清理
        let inner = manager.state_store.get();
        assert!(inner.tasks.is_empty(), "停止任务后应该没有任务在运行");
        
        // 等待一小段时间让中止的任务完成清理
        sleep(Duration::from_millis(200)).await;
    }
}