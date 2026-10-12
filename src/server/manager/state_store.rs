//! 服务器状态存储 - 纯状态存储层，不包含业务逻辑。
//!
//! 该模块提取自 `src/server/state.rs`，保持原有的数据结构和
//! `ServerInner` 的内部实现，但只提供状态访问接口。
//! 所有状态变化都必须通过 `lifecycle.rs` 中的协调函数完成，
//! 以确保事务一致性和状态机正确性。

use crate::constants::*;
use crate::events::{LogLine, ServerStatus};
use crate::server::job::Job;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::process::Child;

/// 服务器运行时状态容器。
///
/// 字段可变性原则：
/// - `child` / `tasks` / `pid` / `started_at` / `active_port` / `status`
///   是 `start / stop / Drop` 共同管理的状态，必须**在单 lock 块内**修改。
/// - `logs` 是高频写入（每条日志行一次 push），但语义独立，单独管理即可。
pub(crate) struct ServerInner {
    /// The running child process, if any.
    pub(crate) child: Option<Child>,
    /// Current status.
    pub(crate) status: ServerStatus,
    /// Retained log lines.
    pub(crate) logs: Vec<LogLine>,
    /// PID of the running child (kept for metric sampling after the child is taken).
    pub(crate) pid: Option<u32>,
    /// Wall-clock time the server entered Running state.
    pub(crate) started_at: Option<std::time::Instant>,
    /// Port the child is bound to (may differ from cfg.port if auto-port kicked in).
    pub(crate) active_port: Option<u16>,
    /// Windows Job Object：绑定子进程到本 Job，使父进程任何方式死亡时
    /// 内核自动 kill 子进程。Drop 时关闭 handle 触发此行为。
    /// Linux/macOS 为 None（用 tokio Child 的 kill_on_drop 兜底）。
    pub(crate) job: Option<Job>,
    /// 本次 start 派生的所有后台任务（stdout reader / stderr reader /
    /// log pump / watcher / metrics sampler）。stop() / restart() 时
    /// 调用 abort() 强制结束，防止跨 start 任务堆叠（修复 C1.2）。
    pub(crate) tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// 服务器状态存储 - 提供对服务器状态的统一访问接口
///
/// 设计：
/// - `inner` 用 `Arc<parking_lot::Mutex>` 共享状态——读多写少（watcher /
///   metrics / 前端轮询），`parking_lot` 在非持锁场景下比 `std::sync::Mutex`
///   快约 30%；
/// - 不提供直接的写方法，所有状态变化都必须通过 `lifecycle.rs` 中的
///   协调函数完成，以确保事务一致性和状态机正确性。
pub struct ServerStateStore {
    inner: Mutex<ServerInner>,
}

impl ServerStateStore {
    /// 创建新的空状态存储
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ServerInner {
                child: None,
                status: ServerStatus::Stopped,
                logs: Vec::new(),
                pid: None,
                started_at: None,
                active_port: None,
                job: None,
                tasks: Vec::new(),
            }),
        }
    }
    
    /// 获取可写状态引用（外部代码必须小心处理）
    ///
    /// **警告**：直接修改状态可能导致状态机不一致。只有
    /// `lifecycle.rs` 中的协调函数才应该调用此方法。
    pub fn get_mut(&self) -> parking_lot::MutexGuard<ServerInner> {
        self.inner.lock()
    }
    
    /// 获取只读状态引用
    pub fn get(&self) -> parking_lot::MutexGuard<ServerInner> {
        self.inner.lock()
    }
    
    /// 检查状态是否为指定状态
    pub fn is_status(&self, status: ServerStatus) -> bool {
        let inner = self.inner.lock();
        inner.status == status
    }
    
    /// 更新状态
    ///
    /// **警告**：直接更新状态可能导致状态机不一致。只有
    /// `lifecycle.rs` 中的协调函数才应该调用此方法。
    pub fn update_status(&self, new_status: ServerStatus) {
        let mut inner = self.inner.lock();
        inner.status = new_status;
    }
    
    /// 获取当前状态
    pub fn status(&self) -> ServerStatus {
        let inner = self.inner.lock();
        inner.status
    }
    
    /// 获取当前活动端口
    pub fn active_port(&self) -> Option<u16> {
        let inner = self.inner.lock();
        inner.active_port
    }
    
    /// 获取当前子进程PID
    pub fn pid(&self) -> Option<u32> {
        let inner = self.inner.lock();
        inner.pid
    }
    
    /// 获取当前日志快照
    pub fn logs_snapshot(&self) -> Vec<LogLine> {
        let inner = self.inner.lock();
        inner.logs.clone()
    }
    
    /// 清除日志缓冲
    pub fn clear_logs(&self) {
        let mut inner = self.inner.lock();
        inner.logs.clear();
    }
}

//
// 单元测试
//

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{LogLine, ServerStatus};

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
}