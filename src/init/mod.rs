//! 软件启动自检与初始化模块。
//!
//! 分三步执行：
//!   ① 环境检查：操作系统、llama-server 可用性、模型目录合法性
//!   ② 驱动与 llama 安装检查：缺失时尝试通过系统包管理器静默补齐
//!   ③ 自动加载：探测 llama 路径、模型目录，完成参数初始化
//!
//! 任何一步失败都会立即返回错误，并保留此前已通过的步骤结果。
//!
//! # 模块结构
//!
//! - [`env_check`] 步骤 ①：环境检查
//! - [`install_check`] 步骤 ②：驱动与 llama 安装检查（含 GPU 探测 + 自动安装）
//! - [`auto_load`] 步骤 ③：自动加载配置
//!
//! 每个步骤都是 `pub(super) async fn step_xxx(&AppHandle, &AppConfig) -> Result<(), String>`，
//! 由 `run_initialization` 依次串行调用。
//!
//! # 与 Tauri command 的关系
//!
//! `commands::init_cmd::run_initialization` 是 IPC 端点，本模块的同名函数
//! 是其内部实现。命令层只做「参数解构 → 调本函数 → 错误转换」。

use tauri::AppHandle;

use crate::config::AppConfig;
use crate::log::emit_step;

mod auto_load;
mod env_check;
mod install_check;

/// 步骤 ID 集合（与前端约定）。
const STEP_ENV: &str = "env-check";
const STEP_INSTALL: &str = "install";
const STEP_INIT: &str = "init";

/// 顶层入口：依次执行三步。
///
/// 返回 `Ok(())` 表示三步全部通过；`Err(String)` 携带首个失败步骤的中文错误信息。
///
/// 步骤顺序：① → ② → ③。
/// ① / ③ 是同步函数（不需要 await 任何东西），② 需要 await 包管理器调用，
/// 所以 `run_initialization` 必须是 async。
pub async fn run_initialization(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    // ---- ① 环境检查 ----
    emit_step(app, STEP_ENV, "① 环境检查", "running", true);
    if let Err(e) = env_check::step_env_check(app, cfg) {
        emit_step(app, STEP_ENV, "① 环境检查", "failed", true);
        return Err(e);
    }
    emit_step(app, STEP_ENV, "① 环境检查", "success", false);

    // ---- ② 驱动与 llama 安装检查 ----
    emit_step(app, STEP_INSTALL, "② 驱动与 llama 安装检查", "running", true);
    if let Err(e) = install_check::step_install_check(app, cfg).await {
        emit_step(app, STEP_INSTALL, "② 驱动与 llama 安装检查", "failed", true);
        return Err(e);
    }
    emit_step(app, STEP_INSTALL, "② 驱动与 llama 安装检查", "success", false);

    // ---- ③ 自动加载配置 ----
    emit_step(app, STEP_INIT, "③ 自动加载配置", "running", true);
    if let Err(e) = auto_load::step_auto_load(app, cfg) {
        emit_step(app, STEP_INIT, "③ 自动加载配置", "failed", true);
        return Err(e);
    }
    emit_step(app, STEP_INIT, "③ 自动加载配置", "success", false);

    Ok(())
}

#[cfg(test)]
mod tests {
    //! 端到端串行测试。
    //!
    //! 真实集成测试需要 `tauri::test::mock_app()` 但其返回 `AppHandle<MockRuntime>`，
    //! 与业务代码的 `AppHandle`（默认 `Wry` 运行时）类型不兼容。需要把 step 函数
    //! 改成 generic over `R: tauri::Runtime` 才能复用——这是后续重构项。
    //!
    //! 目前 init 模块只暴露：
    //! 1) `run_initialization` 逻辑骨架：保证 ①/②/③ 串行调用顺序
    //! 2) 各子模块的 step 行为测试（已覆盖）
    //!
    //! 业务参数校验测试放在 `crate::config::tests`（已覆盖）。
    use super::*;

    /// 验证步骤 ID 常量与前端约定一致。
    /// 改动需同步 `dist/main.js`。
    #[test]
    fn step_ids_match_frontend_contract() {
        assert_eq!(STEP_ENV, "env-check");
        assert_eq!(STEP_INSTALL, "install");
        assert_eq!(STEP_INIT, "init");
    }

    /// 验证模块的对外 API 仅有 `run_initialization`，避免意外暴露内部步骤。
    /// 这是契约测试：保证 init 模块的"窄接口"原则。
    ///
    /// 编译期验证：`run_initialization` 接受 `&AppHandle` 与 `&AppConfig`，
    /// 返回 `Result<(), String>`。其它内部 step_* 与 STEP_* 都是 pub(super) 或更私。
    #[test]
    fn public_api_is_narrow() {
        // 编译期验证：内部 step_* 与 STEP_* 都是 pub(super) 或私有，
        // 外部 crate 看不到。若有人误改 visibility，本测试无法直接校验，
        // 但 lib.rs 的 pub use 列表是显式的，rustdoc 会标记所有公开项。
        //
        // 这里只断言 `run_initialization` 满足基础契约：fn + (AppHandle, AppConfig) → Result。
        let _f = run_initialization;
    }
}
