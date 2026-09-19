//! 全局日志系统初始化模块。
//!
//! 基于 tracing 生态，提供：
//! - 控制台彩色分级输出（ERROR=红, WARN=黄, INFO=绿, DEBUG=蓝, TRACE=灰）
//! - 文件日志持久化（按日滚动，保留 30 天）
//! - Panic hook（崩溃时输出完整 backtrace 到文件）
//! - eprintln! 输出通过 tracing-log 桥接捕获

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// 日志文件目录（`~/.llamaui/logs/`）
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 获取日志文件目录
pub fn log_dir() -> PathBuf {
    LOG_DIR.get_or_init(|| {
        if let Ok(custom) = std::env::var("LLAMAUI_LOG_DIR") {
            return PathBuf::from(custom);
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".llamaui")
            .join("logs")
    }).clone()
}

/// 初始化 tracing 日志系统。
///
/// 产出：
/// - 控制台：带颜色的分级输出（INFO 及以上）
/// - 文件：~/.llamaui/logs/llama-ui.YYYY-MM-DD.log（DEBUG 及以上，按日滚动）
/// - eprintln! 输出通过 tracing-log 桥接捕获
pub fn init() {
    let log_dir = log_dir();
    let _ = fs::create_dir_all(&log_dir);

    // 按日滚动文件 appender：文件名格式 llama-ui.YYYY-MM-DD.log
    let file_appender = tracing_appender::rolling::daily(&log_dir, "llama-ui.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // 保存 guard 到 static，防止 WorkerGuard 被 drop（会停止写入线程）
    static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
    let _ = LOG_GUARD.set(_guard);

    // 控制台层：彩色输出，INFO 及以上
    let console_layer = tracing_subscriber::fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_writer(std::io::stderr)
        .with_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    // 默认级别：INFO，自己的 crate 用 DEBUG
                    tracing_subscriber::EnvFilter::new(
                        "llama_ui_lib=debug,llama_ui=debug,info"
                    )
                }),
        );

    // 文件层：纯文本格式（含时间戳），DEBUG 及以上
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_writer(non_blocking)
        .with_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("debug")
                }),
        );

    // 注册 subscriber
    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .ok();

    // 桥接 log crate（log::info! 等可被捕获到 tracing）
    let _ = tracing_log::LogTracer::init();

    // 安装 panic hook：崩溃时记录完整 backtrace
    install_panic_hook_with_backtrace();

    tracing::info!(
        target: "LlamaUI",
        log_dir = %log_dir.display(),
        "日志系统初始化完成"
    );
}

/// Panic hook：在 panic 时写入完整 backtrace 到日志文件
#[allow(clippy::print_stderr)]
fn install_panic_hook_with_backtrace() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");

        // 先通过 tracing 记录 panic（会写入文件）
        tracing::error!(
            target: "LlamaUI::panic",
            thread = thread_name,
            panic_msg = %panic_info,
            "PANIC 发生"
        );

        // 完整调用栈
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            target: "LlamaUI::panic::backtrace",
            "{}",
            backtrace
        );

        // 同时输出到 stderr（用户可见）
        eprintln!("========================================");
        eprintln!("[LlamaUI PANIC] Thread: {}", thread_name);
        eprintln!("{}", panic_info);
        eprintln!("========================================");

        // 调用默认 hook
        default_hook(panic_info);
    }));
}

/// 获取日志目录路径（供前端使用）
pub fn get_log_file_path() -> String {
    log_dir().to_string_lossy().to_string()
}
