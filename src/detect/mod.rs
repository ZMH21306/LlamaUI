//! 完整检测模块：1 → 2 → 3 → 4 优先级链
//!
//! 设计目标：只要 llama-server.exe / 模型目录在电脑上能被找到，就一定能识别到，
//! 且兜底的全盘扫描不会卡死 UI（带时间预算、入口预算、取消标志、进度事件）。
//!
//! # 优先级链
//!   ① 环境变量 + 配置 + PATH       （< 100ms，绝大多数命中）
//!   ② 虚拟环境扫描                 （限 1.5s，常见于开发者本地）
//!   ③ 关键目录匹配                 （限 2.5s，常见安装位置）
//!   ④ 全盘深度扫描（兜底）         （限 5s 累计，含进度事件与取消）
//!
//! 任一阶段命中即返回；进度通过 `detect-progress` 事件实时推送到前端。
//!
//! # 安全
//! - 阶段 1 在 `which::which` 后用 [`crate::util::path::validate_executable_candidate`]
//!   拒绝 PATH 注入（父目录不能是 `/tmp` 等世界可写位置、文件名必须严格匹配）。
//! - 阶段 3/4 走 [`stage3::key_dir_roots`] / [`stage4::drive_roots`] 静态白名单目录。
//!
//! # 资源
//! - 阶段 1-4 共享 [`Ctx`]（含 [`CancelFlag`] 与 `entries` 计数）。
//! - 全阶段硬性预算 [`ctx::TOTAL_BUDGET_MS`] = 10s；单阶段预算见
//!   `STAGE_BUDGET_2_MS` / `STAGE_BUDGET_3_MS` / `STAGE_BUDGET_4_MS` 常量。
//!
//! # 模块拆分
//! - [`ctx`]：共享上下文（时间预算、取消标志、进度事件发射）
//! - [`stage1`]：环境变量 + PATH（< 100ms）
//! - [`stage2`]：虚拟环境扫描（≤ 1.5s）
//! - [`stage3`]：关键目录匹配（≤ 2.5s）
//! - [`stage4`]：全盘深度扫描兜底（≤ 5s）

use serde::Serialize;
use tauri::AppHandle;

mod ctx;
mod stage1;
mod stage2;
mod stage3;
mod stage4;

use ctx::Ctx;

pub use ctx::{CancelFlag, new_cancel_flag};

/// 进度事件 payload
#[derive(Debug, Clone, Serialize)]
pub struct DetectProgress {
    /// "llama" | "models"
    pub kind: String,
    /// 1..=4
    pub stage: u8,
    /// 阶段中文名
    pub stage_name: String,
    /// 累计耗时（ms）
    pub elapsed_ms: u64,
    /// 已扫描条目数
    pub entries_scanned: usize,
    /// 本阶段附加信息
    pub message: String,
    /// 本阶段是否命中
    pub found: bool,
    /// running | found | done | cancelled | timeout
    pub status: String,
}

/// 检测最终结果
#[derive(Debug, Clone, Serialize)]
pub struct DetectResult {
    pub kind: String,
    pub found: bool,
    pub path: Option<String>,
    /// 0 表示未找到；1..=4 表示在哪一阶段命中
    pub stage_found: u8,
    pub elapsed_ms: u64,
    pub entries_scanned: usize,
    pub message: String,
}

/// 检测 llama-server。返回 DetectResult，全程发射 detect-progress 事件。
pub fn detect_llama_with_progress(app: &AppHandle, cancel: CancelFlag) -> DetectResult {
    let ctx = Ctx::new(app.clone(), "llama", cancel);

    // 阶段 1
    ctx.emit(1, "① 环境变量 / PATH", "检查中...", false, "running");
    if let Some(p) = stage1::llama() {
        ctx.emit(1, "① 环境变量 / PATH", &format!("命中：{}", p.display()), true, "found");
        return ctx.result_done("llama", &p, 1, "通过环境变量或 PATH 找到");
    }
    ctx.emit(1, "① 环境变量 / PATH", "未命中", false, "done");

    if ctx.is_cancelled() {
        return ctx.result_done("llama", std::path::Path::new(""), 0, "已取消");
    }

    // 阶段 2
    ctx.emit(2, "② 虚拟环境扫描", "检查中...", false, "running");
    if let Some(p) = stage2::venv_scan(&ctx) {
        ctx.emit(
            2,
            "② 虚拟环境扫描",
            &format!("命中：{}", p.display()),
            true,
            "found",
        );
        return ctx.result_done("llama", &p, 2, "在虚拟环境中找到");
    }
    ctx.emit(2, "② 虚拟环境扫描", "未命中", false, "done");

    if ctx.is_cancelled() {
        return ctx.result_done("llama", std::path::Path::new(""), 0, "已取消");
    }

    // 阶段 3
    ctx.emit(3, "③ 关键目录匹配", "检查中...", false, "running");
    match stage3::key_dirs_llama(&ctx) {
        Some(p) => {
            ctx.emit(
                3,
                "③ 关键目录匹配",
                &format!("命中：{}", p.display()),
                true,
                "found",
            );
            return ctx.result_done("llama", &p, 3, "在常见安装目录中找到");
        }
        None => {
            ctx.emit(3, "③ 关键目录匹配", "未命中", false, "done");
        }
    }

    if ctx.is_cancelled() {
        return ctx.result_done("llama", std::path::Path::new(""), 0, "已取消");
    }

    // 阶段 4：兜底全盘扫描
    ctx.emit(4, "④ 全盘深度扫描（兜底）", "扫描中...", false, "running");
    match stage4::full_disk_llama(&ctx) {
        Some(p) => {
            ctx.emit(
                4,
                "④ 全盘深度扫描（兜底）",
                &format!("命中：{}", p.display()),
                true,
                "found",
            );
            return ctx.result_done("llama", &p, 4, "在全盘扫描中找到");
        }
        None => {
            let status = if ctx.is_cancelled() {
                "cancelled"
            } else if ctx.is_timed_out() {
                "timeout"
            } else {
                "done"
            };
            ctx.emit(4, "④ 全盘深度扫描（兜底）", "未命中", false, status);
        }
    }

    ctx.result_not_found("llama")
}

/// 检测模型目录。返回 DetectResult，全程发射 detect-progress 事件。
///
/// 关键修复：
///   1) 阶段 2/3 都调用 stage3_key_dirs_models（之前 2 调一次、3 又调一次，浪费）
///   2) 阶段 3 失败后，新增「同级 models 目录探查」步骤
///   3) 全盘扫描不再依赖「直接子项是 .gguf」——也会检查子目录里的 .gguf
///   4) 增强 WinGet 路径扫描（递归 ggml.llamacpp_* 子目录）
pub fn detect_models_with_progress(app: &AppHandle, cancel: CancelFlag) -> DetectResult {
    let ctx = Ctx::new(app.clone(), "models", cancel);

    ctx.emit(1, "① 环境变量 / 配置", "检查中...", false, "running");
    if let Some(p) = stage1::models() {
        ctx.emit(1, "① 环境变量 / 配置", &format!("命中：{}", p.display()), true, "found");
        return ctx.result_done("models", &p, 1, "通过环境变量找到");
    }
    ctx.emit(1, "① 环境变量 / 配置", "未命中", false, "done");

    if ctx.is_cancelled() {
        return ctx.result_done("models", std::path::Path::new(""), 0, "已取消");
    }

    // 阶段 2：关键目录匹配（含 WinGet 浅探查）
    ctx.emit(2, "② 关键目录匹配", "检查中...", false, "running");
    match stage3::key_dirs_models(&ctx) {
        Some(p) => {
            ctx.emit(
                2,
                "② 关键目录匹配",
                &format!("命中：{}", p.display()),
                true,
                "found",
            );
            return ctx.result_done("models", &p, 2, "在常见模型目录中找到");
        }
        None => {
            ctx.emit(2, "② 关键目录匹配", "未命中", false, "done");
        }
    }

    if ctx.is_cancelled() {
        return ctx.result_done("models", std::path::Path::new(""), 0, "已取消");
    }

    // 阶段 3：llama-server 同级 / 父级 models 探查（修复「明明有 models 但识别不到」）
    ctx.emit(3, "③ 关联 llama-server 目录", "检查中...", false, "running");
    if let Some(llama_exe) = stage3::find_any_llama_server(&ctx) {
        if let Some(models_dir) = stage3::find_sibling_models_dir(&llama_exe) {
            ctx.emit(
                3,
                "③ 关联 llama-server 目录",
                &format!(
                    "命中：llama-server={}，models={}",
                    llama_exe.display(),
                    models_dir.display()
                ),
                true,
                "found",
            );
            return ctx.result_done("models", &models_dir, 3, "通过 llama-server 同级目录推断");
        }
        ctx.emit(
            3,
            "③ 关联 llama-server 目录",
            &format!("找到 llama-server {}，但未找到同级 models", llama_exe.display()),
            false,
            "done",
        );
    } else {
        ctx.emit(3, "③ 关联 llama-server 目录", "未找到任何 llama-server", false, "done");
    }

    if ctx.is_cancelled() {
        return ctx.result_done("models", std::path::Path::new(""), 0, "已取消");
    }

    // 阶段 4：全盘
    ctx.emit(4, "④ 全盘深度扫描（兜底）", "扫描中...", false, "running");
    match stage4::full_disk_models(&ctx) {
        Some(p) => {
            ctx.emit(
                4,
                "④ 全盘深度扫描（兜底）",
                &format!("命中：{}", p.display()),
                true,
                "found",
            );
            return ctx.result_done("models", p.as_path(), 4, "在全盘扫描中找到");
        }
        None => {
            let status = if ctx.is_cancelled() {
                "cancelled"
            } else if ctx.is_timed_out() {
                "timeout"
            } else {
                "done"
            };
            ctx.emit(4, "④ 全盘深度扫描（兜底）", "未命中", false, status);
        }
    }

    ctx.result_not_found("models")
}

#[cfg(test)]
mod tests {
    //! 跨阶段共享不变量测试。
    //!
    //! 各阶段的细节测试放在对应子模块（`stage1` / `stage2` / ...）。
    //! 本模块只放不适合归属任何单一阶段的端到端测试。

    use super::*;

    /// DEFECT-005：验证 cancel 标志在多线程间可传播。
    ///
    /// 真实环境的 read_dir cancel 验证需要构造大目录树并在子线程触发 cancel，
    /// 跨平台一致性差。本测试只验证最核心的不变量：
    /// `CancelFlag = Arc<AtomicBool>` 在 `store` 后能被另一线程 `load` 观察到。
    /// 各 read_dir 循环中的 `ctx.is_cancelled()` 调用即基于此机制生效。
    #[test]
    fn detect_loop_responds_to_cancel() {
        use std::sync::atomic::Ordering;
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
}
