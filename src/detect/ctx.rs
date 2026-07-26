//! 共享检测上下文：时间预算、取消标志、进度事件发射、结果构造。
//!
//! 设计动机：
//! - 阶段 1-4 共享同一份 `Ctx`（含 `start` / `start_per_stage` / `entries` /
//!   `cancel` / `app` / `kind`）。
//! - 进度事件发射、取消检查、入口预算消费是所有阶段的通用操作，集中在
//!   `Ctx` 的方法里可以避免各阶段重复 boilerplate。
//! - `result_done` / `result_not_found` 把"从 Ctx 状态构造 DetectResult"
//!   收拢在 Ctx 上，调用方写起来更短。
//!
//! # 时间预算
//! - [`STAGE_BUDGET_2_MS`] / [`STAGE_BUDGET_3_MS`] / [`STAGE_BUDGET_4_MS`]：
//!   单阶段累计时间上限。`check_deadline` 在每个 read_dir 循环中检查，
//!   超时立即返回，**不等待**阶段结束。
//! - [`TOTAL_BUDGET_MS`]：全阶段累计时间硬上限。阶段 1 没有显式 budget
//!   （通常 < 100ms 命中），2-4 总和不超过此值。
//!
//! # 入口预算
//! - [`MAX_ENTRIES`]：单次检测允许访问的目录/文件条目数上限。`try_consume`
//!   在每次 read_dir / stat 之前消费，**超额立即返回**。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tauri::{AppHandle, Emitter};

use super::{DetectProgress, DetectResult};

/// 全局取消标志：每个 AppState 持有一份；点击"取消"置 true。
pub type CancelFlag = Arc<AtomicBool>;

/// 单阶段时间预算（毫秒）。含累计预算，超过即中止。
pub(crate) const STAGE_BUDGET_2_MS: u128 = 1_500;
pub(crate) const STAGE_BUDGET_3_MS: u128 = 2_500;
pub(crate) const STAGE_BUDGET_4_MS: u128 = 5_000;
/// 全阶段累计硬上限（兜底，超此值必停）
pub const TOTAL_BUDGET_MS: u128 = 10_000;
/// 入口预算：单次检测允许访问的目录/文件条目数上限
pub(crate) const MAX_ENTRIES: usize = 30_000;
/// 单个目录展开的最大子项数（防止 Program Files 之类的大目录拖慢）
pub(crate) const PER_DIR_LIMIT: usize = 800;

/// 创建新的取消标志（包装 `Arc<AtomicBool>` 的工厂函数）。
pub fn new_cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

/// 共享检测上下文：所有阶段共享同一份实例。
///
/// 注意：本类型无法在单元测试中直接构造（需要 `tauri::AppHandle`，
/// 而项目未启用 `tauri/test` 特性）。`Ctx` 的内部不变量通过
/// 端到端检测命令与 `new_cancel_flag` 单测间接覆盖。
pub(crate) struct Ctx {
    start: Instant,
    entries: AtomicUsize,
    cancel: CancelFlag,
    app: AppHandle,
    kind: String,
    /// 各阶段开始时间（按 stage 1..=4 索引）。用于 `stage_elapsed`。
    start_per_stage: [Instant; 5],
}

impl Ctx {
    /// 创建新上下文。所有 stage 开始时间初始化为 `now`。
    pub fn new(app: AppHandle, kind: &str, cancel: CancelFlag) -> Self {
        let now = Instant::now();
        Self {
            start: now,
            entries: AtomicUsize::new(0),
            cancel,
            app,
            kind: kind.into(),
            start_per_stage: [now; 5],
        }
    }

    /// 从此刻起的累计耗时（ms）。
    pub fn elapsed(&self) -> u128 {
        self.start.elapsed().as_millis()
    }

    /// 指定阶段的耗时（ms）。stage ∈ 1..=4。
    pub fn stage_elapsed(&self, stage: u8) -> u128 {
        self.start_per_stage[stage as usize].elapsed().as_millis()
    }

    /// 是否被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 是否已超过全阶段累计预算。
    pub fn is_timed_out(&self) -> bool {
        self.elapsed() > TOTAL_BUDGET_MS
    }

    /// 尝试消费一个入口额度；返回 false 表示已超额。
    pub fn try_consume(&self) -> bool {
        let prev = self.entries.fetch_add(1, Ordering::Relaxed);
        prev < MAX_ENTRIES
    }

    /// 当前已消费条目数。
    pub fn entries(&self) -> usize {
        self.entries.load(Ordering::Relaxed)
    }

    /// 发射 `detect-progress` 事件到前端。
    pub fn emit(&self, stage: u8, stage_name: &str, message: &str, found: bool, status: &str) {
        let _ = self.app.emit(
            "detect-progress",
            DetectProgress {
                kind: self.kind.clone(),
                stage,
                stage_name: stage_name.into(),
                elapsed_ms: self.elapsed() as u64,
                entries_scanned: self.entries(),
                message: message.into(),
                found,
                status: status.into(),
            },
        );
    }

    /// 检查取消 / 全阶段超时 / 当前阶段预算。
    /// 返回 `Err("cancelled")` / `Err("timeout")` / `Err("stage-N budget exceeded")`
    /// 时调用方应立即返回，**不**继续做任何 read_dir。
    pub fn check_deadline(&self, stage: u8) -> Result<(), String> {
        if self.is_cancelled() {
            return Err("cancelled".into());
        }
        if self.elapsed() > TOTAL_BUDGET_MS {
            return Err("timeout".into());
        }
        let budget = match stage {
            2 => STAGE_BUDGET_2_MS,
            3 => STAGE_BUDGET_3_MS,
            4 => STAGE_BUDGET_4_MS,
            _ => return Ok(()), // 阶段 1 无显式 budget
        };
        if self.stage_elapsed(stage) > budget {
            return Err(format!("stage-{} budget exceeded", stage));
        }
        Ok(())
    }

    /// 构造命中版本的 `DetectResult`。
    pub fn result_done(
        &self,
        kind: &str,
        path: &std::path::Path,
        stage: u8,
        message: &str,
    ) -> DetectResult {
        DetectResult {
            kind: kind.into(),
            found: true,
            path: Some(path.to_string_lossy().into_owned()),
            stage_found: stage,
            elapsed_ms: self.elapsed() as u64,
            entries_scanned: self.entries(),
            message: message.into(),
        }
    }

    /// 构造未命中版本的 `DetectResult`。
    pub fn result_not_found(&self, kind: &str) -> DetectResult {
        DetectResult {
            kind: kind.into(),
            found: false,
            path: None,
            stage_found: 0,
            elapsed_ms: self.elapsed() as u64,
            entries_scanned: self.entries(),
            message: "在所有阶段均未找到".into(),
        }
    }
}

/// 缓存：单次进程内只算一次关键目录根列表。
///
/// 用 `OnceLock` 避免每次检测（用户连点几次）都重新构造 `Vec` + 调 `dirs::*` API。
pub(crate) fn cached_path_bufs<F: FnOnce() -> Vec<PathBuf>>(build: F) -> &'static [PathBuf] {
    static CACHE: OnceLock<Vec<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(build)
}

#[cfg(test)]
mod tests {
    //! `CancelFlag` 跨线程可见性 + `new_cancel_flag` 工厂冒烟测试。
    //!
    //! `Ctx` 本身需要 `AppHandle` 才能构造，而本项目未启用 `tauri/test`
    //! 特性（项目记忆：mock_app 返回 `AppHandle<MockRuntime>` 与业务代码
    //! `AppHandle<Wry>` 类型不兼容）。`Ctx` 的内部不变量通过
    //! 端到端 `detect_*_with_progress` 调用间接覆盖。
    use super::*;
    use std::sync::atomic::Ordering;

    /// 工厂函数：返回的 `CancelFlag` 在 store 后能被另一线程 load 观察到。
    #[test]
    fn cancel_flag_spawn_thread_visibility() {
        let cancel: CancelFlag = new_cancel_flag();
        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cancel_clone.store(true, Ordering::Relaxed);
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            cancel.load(Ordering::Relaxed),
            "cancel 标志在 spawn 线程 store 后应被主线程 load 看到"
        );
    }

    /// 工厂函数：返回的 `CancelFlag` 初始值为 false。
    #[test]
    fn cancel_flag_starts_false() {
        let cancel: CancelFlag = new_cancel_flag();
        assert!(!cancel.load(Ordering::Relaxed));
    }

    /// 验证 `cached_path_bufs` 返回的引用是稳定的（OnceLock 缓存不变量）。
    ///
    /// 重要：`cached_path_bufs` 是**全局**单实例缓存（`static CACHE: OnceLock<...>`），
    /// 任何先于此测试运行并调用了 `cached_path_bufs` 的测试都会让 cache 提前
    /// 初始化，因此本测试**不**断言闭包执行次数。
    /// 只验证「相同调用方必返回相同指针」（这正是缓存的核心契约）。
    #[test]
    fn cached_path_bufs_returns_same_pointer() {
        let r1: &[PathBuf] = cached_path_bufs(|| vec![PathBuf::from("/cached_a")]);
        let r2: &[PathBuf] = cached_path_bufs(|| vec![PathBuf::from("/cached_b")]);
        assert_eq!(r1.as_ptr(), r2.as_ptr(), "OnceLock 缓存必须返回相同指针");
    }
}
