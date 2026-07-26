//! 启动初始化命令。
//!
//! 把 [`crate::init::run_initialization`] 暴露给前端，由前端在 UI 启动时
//! 触发三步初始化流程（环境检查 → 安装检查 → 自动加载）。
//!
//! 每步的执行进度会作为日志组与状态事件发送到前端，前端据此展示
//! 「① 环境检查 → ② 安装检查 → ③ 自动加载」的折叠日志。

use tauri::{AppHandle, State};

use super::AppState;

/// 运行三步初始化流程：环境检查 → 安装检查 → 自动加载。
/// 每步结果会作为日志组与状态事件发送到前端。
#[tauri::command]
pub async fn run_initialization(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let cfg = state.config.get();
    crate::init::run_initialization(&app, &cfg).await
}

#[cfg(test)]
mod tests {
    //! init 业务逻辑的测试位于 `crate::init` 各子模块底部。
    //! 本模块仅作占位，避免空测试模块触发 dead_code 警告。
    #[test]
    fn placeholder() {
        // 命令层只做 (state, app) → crate::init::run_initialization 的转译，
        // 不含独立逻辑。真实测试在 init/env_check / init/install_check / init/auto_load。
    }
}
