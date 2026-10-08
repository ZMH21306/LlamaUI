//! 保留版的 `ServerProcess` 状态存储模块。
//! 
//! 本模块保留了原始的 `ServerProcess` 和 `ServerInner` 实现，
//! 用于向后兼容旧代码。新的代码应该使用 `crate::server::manager::state_store`
//! 模块。

use crate::constants::*;
use crate::events::{LogLine, ServerStatus};
use parking_lot::Mutex;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use crate::server::job::Job;
use tokio::sync::Mutex as TokioMutex;

pub struct ServerInner {
    pub(crate) child: Option<Child>,
    pub(crate) status: ServerStatus,
    pub(crate) logs: Vec<LogLine>,
    pub(crate) pid: Option<u32>,
    pub(crate) started_at: Option<std::time::Instant>,
    pub(crate) active_port: Option<u16>,
    pub(crate) job: Option<Job>,
    pub(crate) tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub struct ServerProcess {
    pub(crate) inner: Arc<Mutex<ServerInner>>,
    pub(crate) start_mutex: Arc<TokioMutex<()>>,
    pub(crate) generation: Arc<AtomicU64>,
}

impl ServerProcess {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ServerInner {
                child: None,
                status: ServerStatus::Stopped,
                logs: Vec::new(),
                pid: None,
                started_at: None,
                active_port: None,
                job: None,
                tasks: Vec::new(),
            })),
            start_mutex: Arc::new(TokioMutex::new(())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
    
    pub fn status(&self) -> ServerStatus {
        let inner = self.inner.lock();
        inner.status
    }
    
    pub fn active_port(&self) -> Option<u16> {
        let inner = self.inner.lock();
        inner.active_port
    }
    
    pub fn pid(&self) -> Option<u32> {
        let inner = self.inner.lock();
        inner.pid
    }
    
    pub fn logs_snapshot(&self) -> Vec<LogLine> {
        let inner = self.inner.lock();
        inner.logs.clone()
    }
    
    pub fn clear_logs(&self) {
        let mut inner = self.inner.lock();
        inner.logs.clear();
    }
}

impl Default for ServerProcess {
    fn default() -> Self {
        Self::new()
    }
}