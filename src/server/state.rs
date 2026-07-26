//! `ServerProcess` 内部状态与轻量访问器。
//!
//! 把以下"纯结构 / 读多写少"的部分单独抽出，使 `lifecycle.rs` 只关心
//! 进程派生与停机（start / stop / kill_orphan_on_drop）。
//!
//! 包含：
//! - [`ServerInner`]：所有运行时状态的容器（含 `Mutex` 共享语义）
//! - [`ServerProcess`]：对外的 `Arc<Mutex<ServerInner>>` + start_mutex
//! - 轻量访问器：`status` / `logs_snapshot` / `clear_logs` / `active_port`
//! - 日志缓冲容量上限 [`MAX_LOG_LINES`]

use parking_lot::Mutex;
use std::mem;
use std::sync::Arc;
use tokio::process::Child;
use tokio::sync::Mutex as TokioMutex;

use crate::events::{LogLine, ServerStatus};

use super::job::Job;

/// Maximum number of log lines retained in memory. Older lines are dropped.
///
/// 与 [`super::log_truncate::MAX_LOG_LINE_BYTES`]（单行字节上限）配合形成
/// 日志缓冲的双层防御：单行字节数 + 总行数。
pub const MAX_LOG_LINES: usize = 5000;

/// 进程运行时状态容器。
///
/// 字段可变性原则：
/// - `child` / `tasks` / `job` / `pid` / `started_at` / `active_port` / `status`
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

/// llama-server 进程管理器的对外句柄。
///
/// 设计：
/// - `inner` 用 `Arc<parking_lot::Mutex>` 共享状态——读多写少（watcher /
///   metrics / 前端轮询），`parking_lot` 在非持锁场景下比 `std::sync::Mutex`
///   快约 30%；
/// - `start_mutex` 用 `Arc<tokio::sync::Mutex>` 串行化 start / stop /
///   restart——必须在 async 上下文持锁，否则会跨 `.await` 持锁导致
///   `Send` 不合规。
pub struct ServerProcess {
    pub(crate) inner: Arc<Mutex<ServerInner>>,
    /// 串行化 start / stop / restart 调用，防止并发导致子进程孤儿泄漏。
    /// 注意是 `tokio::sync::Mutex`（不是 parking_lot），因为调用方都在 async 上下文。
    pub(crate) start_mutex: Arc<TokioMutex<()>>,
}

impl ServerProcess {
    /// 创建空的 `ServerProcess`。所有状态字段为初始值（Stopped / 无子进程）。
    ///
    /// 通过 `Arc::new(Mutex)` 共享内部状态，使得 watcher / metrics / 前端轮询
    /// 可以并发访问。`start_mutex` 用 `tokio::sync::Mutex`（而非 parking_lot），
    /// 因为 start/stop 调用方都在 async 上下文。
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
        }
    }

    /// 当前状态（克隆 `ServerStatus` 枚举，零拷贝）。
    pub fn status(&self) -> ServerStatus {
        self.inner.lock().status
    }

    /// 内存中累积的日志快照（最近最多 [`MAX_LOG_LINES`] 行）。
    /// 返回克隆的 `Vec`，避免锁跨越 IPC 边界。
    pub fn logs_snapshot(&self) -> Vec<LogLine> {
        self.inner.lock().logs.clone()
    }

    /// 清空内存中累积的日志。
    /// 用 `mem::take` 把旧 Vec 弹出后立即 drop 释放内存，避免
    /// 大缓冲（接近 5000 行）清空时的延迟。
    pub fn clear_logs(&self) {
        mem::take(&mut self.inner.lock().logs);
    }

    /// 当前实际绑定的端口（与 `cfg.port` 不同的原因是 auto-port 可能顺延）。
    pub fn active_port(&self) -> Option<u16> {
        self.inner.lock().active_port
    }
}

impl Default for ServerProcess {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        // 当主程序退出（正常 / panic 都可能 drop 不到，但 unwind 时一定会走这里）
        // 时确保子进程不会变成孤儿。
        //
        // 实际的清理逻辑在 `lifecycle::kill_orphan_on_drop`，与 start / stop
        // 共用同一份 "abort tasks + kill child + close job + clear fields"
        // 原子操作。
        super::lifecycle::kill_orphan_on_drop(&self.inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 新建实例的初始状态必须全部为「无」
    #[test]
    fn new_creates_empty_inner_state() {
        let sp = ServerProcess::new();
        let inner = sp.inner.lock();
        assert!(inner.tasks.is_empty(), "新进程 tasks 必须为空");
        assert!(inner.child.is_none(), "新进程 child 必须为 None");
        assert!(inner.pid.is_none(), "新进程 pid 必须为 None");
        assert!(inner.started_at.is_none(), "新进程 started_at 必须为 None");
        assert!(inner.active_port.is_none(), "新进程 active_port 必须为 None");
        assert!(inner.job.is_none(), "新进程 job 必须为 None");
        assert_eq!(inner.status, ServerStatus::Stopped, "新进程 status 必须为 Stopped");
        assert!(inner.logs.is_empty(), "新进程 logs 必须为空");
    }

    /// 初始 status() 返回 Stopped
    #[test]
    fn initial_status_is_stopped() {
        let sp = ServerProcess::new();
        assert_eq!(sp.status(), ServerStatus::Stopped);
    }

    /// 初始 active_port() 返回 None
    #[test]
    fn initial_active_port_is_none() {
        let sp = ServerProcess::new();
        assert_eq!(sp.active_port(), None);
    }

    /// clear_logs 对空缓冲是 no-op
    #[test]
    fn clear_logs_on_empty_is_noop() {
        let sp = ServerProcess::new();
        sp.clear_logs();
        assert!(sp.logs_snapshot().is_empty());
    }

    /// logs_snapshot 是克隆（修改内部不影响返回值）
    #[test]
    fn logs_snapshot_is_cloned() {
        let sp = ServerProcess::new();
        {
            let mut inner = sp.inner.lock();
            inner.logs.push(LogLine {
                timestamp: "2026-01-01 00:00:00".into(),
                stream: "stdout".into(),
                text: "hello".into(),
                group: None,
            });
        }
        let snap = sp.logs_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].text, "hello");
        // 原始缓冲不受影响
        assert_eq!(sp.logs_snapshot().len(), 1);
    }

    /// Default 实现等同于 new()
    #[test]
    fn default_matches_new() {
        let sp1 = ServerProcess::new();
        let sp2 = ServerProcess::default();
        assert_eq!(sp1.status(), sp2.status());
        assert_eq!(sp1.active_port(), sp2.active_port());
    }
}
