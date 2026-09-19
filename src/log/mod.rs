//! 日志模块：前端事件发射 + 后端日志脱敏/初始化。
//!
//! # 子模块
//!
//! - [`emitter`]：统一的事件发射入口（emit_log / emit_step / emit_status）
//!   `log.rs` 中的函数。
//! - [`sanitizer`]：日志脱敏工具（原 `log_sanitizer.rs`）。
//! - [`tracing_setup`]：tracing 全局初始化、文件滚动、panic hook。

pub mod emitter;
pub mod sanitizer;
pub mod tracing_setup;

pub use emitter::{emit_log, emit_log_to, emit_step, emit_status};
pub use sanitizer::sanitize_log;
pub use tracing_setup::get_log_file_path;
pub use tracing_setup::init;