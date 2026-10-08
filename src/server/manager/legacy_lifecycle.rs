//! 保留的旧版生命周期管理模块
//!
//! 该模块向后兼容旧的 `src/server/lifecycle.rs` 提供的启动/停止功能。
//! 新代码应使用 `crate::server::manager::state_store` 和相关任务工厂。

use crate::server::state::ServerProcess;
use crate::events::{LogLine, ServerStatus};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

/// 服务器生命周期管理器 - 保留旧版接口以保持向后兼容
///
/// 包装 [`ServerProcess`] 的生命周期方法，保持原有 API 不变。
pub struct ServerLifecycleManager {
    process: Arc<ServerProcess>,
}

impl ServerLifecycleManager {
    /// 创建新的生命周期管理器
    pub fn new() -> Self {
        Self {
            process: Arc::new(ServerProcess::new()),
        }
    }

    /// 启动服务器（委托给 [`ServerProcess::start`]）
    pub async fn start(&self, app: tauri::AppHandle, config: &crate::config::AppConfig) -> Result<(), anyhow::Error> {
        self.process.start(app, config.clone()).await
    }

    /// 停止服务器（委托给 [`ServerProcess::stop`]）
    pub async fn stop(&self, app: &tauri::AppHandle) -> Result<(), anyhow::Error> {
        self.process.stop(app).await
    }

    /// 强制关闭服务器（兜底清理）
    pub async fn force_close(&self, app: &tauri::AppHandle) -> Result<(), anyhow::Error> {
        // 复用 stop 作为强制关闭的实现
        self.process.stop(app).await
    }

    /// 获取当前服务状态
    pub fn status(&self) -> ServerStatus {
        self.process.status()
    }

    /// 获取当前活动端口
    pub fn active_port(&self) -> Option<u16> {
        self.process.active_port()
    }

    /// 获取日志快照
    pub fn logs_snapshot(&self) -> Vec<LogLine> {
        self.process.logs_snapshot()
    }

    /// 清除日志
    pub fn clear_logs(&self) {
        self.process.clear_logs()
    }

    /// 启动互斥（用于序列化 start/stop/restart 调用）
    pub fn start_mutex(&self) -> &Arc<TokioMutex<()>> {
        &self.process.start_mutex
    }
}