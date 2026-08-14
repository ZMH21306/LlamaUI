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
mod config_io;
mod detect;
mod error;
pub mod error_macros;
mod events;
mod gpu_detect;
mod init;
mod log;
pub mod log_sanitizer;
mod llama_downloader;
mod metrics_enhanced;
mod model_management;
mod plugin_framework;
mod recovery;
mod remote_server;
mod server;
mod tracing_setup;
mod update_check;
mod util;

pub use error::{AppError, ConfigError, DetectError, ProcessError};
pub use events::{LogLine, ServerStatus, StepStatus};
pub use log::{emit_log, emit_log_to, emit_status, emit_step};

use commands::AppState;
use tauri::Emitter;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 1. 初始化 tracing 日志系统（控制台彩色 + 文件滚动 + panic hook）
    //    这必须在所有其他初始化之前，包括旧的 panic hook。
    tracing_setup::init();

    tracing::info!(
        target: "LlamaUI",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "LlamaUI 启动"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                let status = state.server.status();
                if status == crate::events::ServerStatus::Running
                    || status == crate::events::ServerStatus::Starting
                {
                    // 拦截关闭请求，提示用户先停止服务
                    api.prevent_close();
                    let _ = window.emit("close-requested", true);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // 服务进程控制
            commands::server_cmd::start_server,
            commands::server_cmd::stop_server,
            commands::server_cmd::restart_server,
            commands::server_cmd::get_status,
            commands::server_cmd::get_logs,
            commands::server_cmd::clear_logs,
            commands::server_cmd::force_close,
            // 配置读写
            commands::config_cmd::save_config,
            commands::config_cmd::load_config,
            // 自动检测
            commands::detect_cmd::detect_llama_server,
            commands::detect_cmd::detect_models_dir,
            commands::detect_cmd::cancel_detection,
            commands::detect_cmd::check_models_dir,
            // 启动初始化
            commands::init_cmd::run_initialization,
            // 杂项
            commands::system_cmd::open_external_url,
            commands::system_cmd::get_log_dir,
            // 新增功能
            commands::export_cmd::export_logs,
            // 配置导入导出
            commands::config_io_cmd::export_config_json,
            commands::config_io_cmd::export_config_to_file,
            commands::config_io_cmd::import_config_from_file,
            commands::recovery_cmd::get_diagnosis,
            commands::recovery_cmd::auto_fix_issues,
            commands::update_cmd::check_updates,
            commands::update_cmd::cleanup_old_version,
            // llama 自动下载
            commands::download_cmd::download_llama_server,
            commands::download_cmd::detect_gpu,
            commands::download_cmd::list_gpu_backends,
            // GPU 检测与诊断
            commands::gpu_cmd::detect_gpus,
            commands::gpu_cmd::diagnose_gpu,
            // 多模型管理
            commands::model_cmd::list_models,
            commands::model_cmd::filter_models_by_tag,
            commands::model_cmd::refresh_models,
            commands::model_cmd::select_model,
            commands::model_cmd::get_selected_model,
            // 远程服务器管理
            commands::remote_cmd::add_remote_server,
            commands::remote_cmd::remove_remote_server,
            commands::remote_cmd::list_remote_servers,
            commands::remote_cmd::get_remote_server,
            commands::remote_cmd::probe_remote_server,
            // 插件管理
            commands::plugin_cmd::list_plugins,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Tauri 启动失败是最严重的错误：WebView2 没装 / 配置解析失败等。
            tracing::error!(target: "LlamaUI", error = %e, "Tauri 启动失败");
            std::process::exit(1);
        });
}
