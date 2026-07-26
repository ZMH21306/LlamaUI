// Crate root：模块装配 + Tauri Builder。
//
// 重构后模块层次（按职责 + 依赖方向）：
//   - error        统一错误类型（无依赖）
//   - events       事件名常量 + payload 类型（无业务依赖）
//   - log          日志发射统一入口（依赖 events）
//   - util         通用工具（path / time，无业务依赖）
//   - config       配置层（依赖 error）
//   - server       进程管理（原 server/，依赖 config / log / events / util）
//   - detect       自动检测（依赖 events / util）
//   - init         启动初始化（依赖 server / detect / config / log）
//   - commands     Tauri command 适配层（依赖以上所有）
//     ├─ mod.rs           AppState + 共享测试
//     ├─ server_cmd.rs    服务进程控制（start/stop/restart/status/logs）
//     ├─ config_cmd.rs    配置读写
//     ├─ detect_cmd.rs    自动检测（detect/cancel/check_models_dir）
//     ├─ init_cmd.rs      启动初始化
//     └─ system_cmd.rs    杂项（open_external_url）

// 测试代码 lint 例外：以下 lint 在测试中属于正常模式（`unwrap` / `expect` / `panic!`
// 是测试断言的常见形式）。仅在 cfg(test) 下开启豁免，production 代码不受影响。
#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::expect_fun_call,
        clippy::bool_assert_comparison
    )
)]

mod commands;
mod config;
mod detect;
mod error;
mod events;
mod init;
mod log;
mod server;
mod util;

pub use error::{AppError, ConfigError, DetectError, ProcessError};
pub use events::{LogLine, ServerStatus, StepStatus};
pub use log::{emit_log, emit_log_to, emit_status, emit_step};

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[allow(clippy::print_stderr)] // Tauri 启动失败时 WebView 不可用，stderr 是唯一输出通道
pub fn run() {
    // 安装自定义 panic hook：在 panic 时（无论 panic=abort 还是 unwind），
    // 把 panic 信息打出来。子进程清理由 ServerProcess::drop() 兜底。
    install_panic_cleanup_hook();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::server_cmd::start_server,
            commands::server_cmd::stop_server,
            commands::server_cmd::restart_server,
            commands::server_cmd::get_status,
            commands::server_cmd::get_logs,
            commands::server_cmd::clear_logs,
            commands::config_cmd::save_config,
            commands::config_cmd::load_config,
            commands::detect_cmd::detect_llama_server,
            commands::detect_cmd::detect_models_dir,
            commands::detect_cmd::cancel_detection,
            commands::detect_cmd::check_models_dir,
            commands::init_cmd::run_initialization,
            commands::system_cmd::open_external_url,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Tauri 启动失败是最严重的错误：WebView2 没装 / 配置解析失败等。
            // 此时 WebView 还没起来，只能走 stderr + 退出码 1。
            eprintln!("[LlamaUI] 启动失败：{}", e);
            std::process::exit(1);
        });
}

/// 进程级 panic hook：在 panic 发生时（先于 abort / unwind 展开）打 trace 到 stderr。
///
/// 子进程清理的兜底：`ServerProcess::Drop` 在 Arc 引用计数归零时调用
/// `kill_orphan_on_drop()`，向子进程发 SIGKILL/TerminateProcess，避免主进程崩溃后
/// llama-server 仍占着 GPU 显存与端口。
#[allow(clippy::print_stderr)] // panic 时 WebView 不可用，stderr 是唯一通道
fn install_panic_cleanup_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        eprintln!("[LlamaUI panic] {}", panic_info);
        // 调用默认 hook（生成 trace / dump）
        default_hook(panic_info);
    }));
}
