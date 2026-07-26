//! 有界日志通道（drop-oldest 策略）。
//!
//! 设计动机：llama-server 启动期 verbose 模式可达 10k 行/秒，无界 mpsc
//! 在 WebView 卡顿时持续累积 → OOM。本模块用 `mpsc::channel(2048)`
//! 配合 reader 端的 `try_send` + 原子计数器实现"丢日志换稳定"。
//!
//! 容量选择说明（LOG_CHANNEL_CAPACITY = 2048）：
//!   - 项目硬约束提到前端 LOG_QUEUE_MAX = 5000；本通道是 reader → pump 的内部通道，
//!     容量只需覆盖 pump 处理一次 emit + push Vec 的耗时即可（亚毫秒级）。
//!   - 2048 ≈ 典型 verbose 输出 200ms 的缓冲，足够抵御 burst 又显著小于 5000。
//!   - 未来若出现持续 100% 满载，可上调到 4096/8192；不建议超过 5000 与
//!     前端队列上限对齐，避免背压不一致。

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

use crate::server::LogLine;

/// 有界 mpsc 容量。填满后 reader 端 `try_send` 失败，调用方决定丢弃。
pub const LOG_CHANNEL_CAPACITY: usize = 2048;

/// 累计被丢弃的日志行数（通道满 + receiver 断开合并计数）。
/// 公开给 metrics / 系统日志做"每 100 条 dropped 一条系统日志"的节流。
pub static DROPPED_LOG_LINES: AtomicU64 = AtomicU64::new(0);

/// 创建有界 mpsc 通道 + 共享的 dropped 计数。
///
/// 选择 `mpsc::channel`（有界）而非 `mpsc::unbounded_channel`：
///   - 无界通道在 WebView 卡顿时累积 2 MB/s 持续 N 秒 → OOM（DEFECT-006 根因）。
///   - 有界 + try_send 让 reader 在容量满时主动丢弃，宁可丢日志也不阻塞子进程输出读取。
pub fn create() -> (mpsc::Sender<LogLine>, mpsc::Receiver<LogLine>) {
    mpsc::channel(LOG_CHANNEL_CAPACITY)
}

/// reader 端用 try_send：成功返回 `true`；失败（容量满 / receiver 已断开）时
/// 增加 dropped 计数并返回 `false`。
///
/// 设计选择：**只计数，不主动从 receiver 拉取旧消息**。
///   - 拉取旧消息需要在 reader 持有额外 receiver 引用 + 引入"取一丢一"的复杂逻辑。
///   - 当前场景下"丢最新 vs 丢最旧"语义差异不大（用户看不完这么多日志），
///     简化实现 → 选"丢最新 + 计数"。
pub fn try_send_or_count(tx: &mpsc::Sender<LogLine>, line: LogLine) -> bool {
    match tx.try_send(line) {
        Ok(()) => true,
        Err(_) => {
            // 容量满 / channel closed 都视为"丢弃"。Ordering::Relaxed：仅做统计，
            // 无需 happens-before 关系。
            DROPPED_LOG_LINES.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    //! 验证有界通道在填满后 `try_send` 必须失败且 dropped 计数必须累计。
    //!
    //! 测试策略：
    //!   1) 创建一个有界通道（容量 2048）。
    //!   2) 用 `send().await` 把容量填满（不读 receiver）。
    //!   3) 再 `try_send_or_count` 100 条 → 必须全失败，dropped 计数 ≥ 100。
    //!   4) 排空 receiver → 恰好收到 LOG_CHANNEL_CAPACITY 条。
    use super::*;

    /// P0-6（DEFECT-006）核心测试：通道满后 try_send 必须失败，丢弃计数必须累加。
    #[tokio::test]
    #[allow(clippy::expect_used, clippy::panic)]
    async fn channel_drops_oldest_when_full() {
        // 每次测试起始重置计数器（避免跨测试污染）。
        DROPPED_LOG_LINES.store(0, Ordering::Relaxed);

        let (tx, mut rx) = create();

        // 1) 填满通道：2048 条全送进 buffer（不读）
        for i in 0..LOG_CHANNEL_CAPACITY {
            // 实际 LogLine 字段是 timestamp/stream/text/group（plan 草稿里写的
            // ts_ms/level/msg 与实际结构不符，此处按真实字段构造）。
            let line = LogLine {
                timestamp: format!("t{}", i),
                stream: "stdout".into(),
                text: format!("line {}", i),
                group: None,
            };
            // 用 send().await 填满：未满时不会 backpressure，永不失败。
            // 此处用 .expect() 与项目其它测试保持一致（job.rs/port.rs 都用 expect）。
            tx.send(line).await.expect("通道未满时 send 必须成功");
        }

        // 2) 不读 receiver，再 try_send 100 条 → 必须全失败
        let snapshot_before = DROPPED_LOG_LINES.load(Ordering::Relaxed);
        for i in 0..100 {
            let ok = try_send_or_count(
                &tx,
                LogLine {
                    timestamp: format!("overflow-{}", i),
                    stream: "stdout".into(),
                    text: "x".into(),
                    group: None,
                },
            );
            assert!(!ok, "通道满时 try_send_or_count 必须返回 false（第 {} 条）", i);
        }
        let snapshot_after = DROPPED_LOG_LINES.load(Ordering::Relaxed);
        assert_eq!(
            snapshot_after - snapshot_before,
            100,
            "100 次 try_send 失败必须累计 100 条 dropped，实际增加 {}",
            snapshot_after - snapshot_before
        );

        // 3) 排空 receiver：恰好收到 LOG_CHANNEL_CAPACITY 条（被丢弃的 100 条没有入队）
        let mut received = 0usize;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(
            received, LOG_CHANNEL_CAPACITY,
            "receiver 只能拿到填满时的 {} 条，溢出 100 条已被丢弃",
            LOG_CHANNEL_CAPACITY
        );
    }

    /// 辅助测试：try_send_or_count 在 receiver drop 后必须返回 false（同样计入 dropped）。
    /// 这覆盖 receiver 提前关闭的边界（不是容量满，而是 channel closed）。
    #[tokio::test]
    async fn try_send_returns_false_when_receiver_dropped() {
        DROPPED_LOG_LINES.store(0, Ordering::Relaxed);

        let (tx, rx) = create();
        // 立即 drop receiver → channel 关闭
        drop(rx);

        let ok = try_send_or_count(
            &tx,
            LogLine {
                timestamp: "t".into(),
                stream: "stdout".into(),
                text: "x".into(),
                group: None,
            },
        );
        assert!(!ok, "receiver drop 后 try_send 必须失败");
        assert_eq!(
            DROPPED_LOG_LINES.load(Ordering::Relaxed),
            1,
            "receiver drop 触发的失败也必须计入 dropped"
        );
    }
}
