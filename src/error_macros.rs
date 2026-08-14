//! 应用级通用错误处理宏与工具。
//!
//! # 设计目标
//!
//! - 提供 `command_err!` 宏，统一 `Result<_, String>` 的 Tauri 命令错误转换，
//!   减少 `.map_err(|e| e.to_string())` 的重复代码。
//! - 提供 `user_friendly_msg!` 宏，把内部技术错误转成用户可读的中文提示。
//!
//! # 使用示例
//!
//! ```rust,ignore
//! #[tauri::command]
//! fn do_something(input: String) -> Result<(), String> {
//!     let value = parse_value(&input)?; // anyhow::Error → CommandError
//!     Ok(())
//! }
//! ```

/// Tauri 命令统一错误转换宏。
///
/// 把 `anyhow::Error` 或自定义错误类型转换为 `String`，避免命令函数中大量
/// 的 `.map_err(|e| e.to_string())` 样板代码。
///
/// # 示例
///
/// ```rust,ignore
/// fn my_command() -> Result<String, String> {
///     some_fallible_operation().command_err()?;
///     Ok("ok".to_string())
/// }
/// ```
///
/// 在 nightly clippy 上会被识别为不需要单独写 `.map_err()`。
#[macro_export]
macro_rules! command_err {
    ($expr:expr) => {{
        $expr.map_err(|e: anyhow::Error| e.to_string())
    }};
    ($expr:expr, $msg:expr) => {{
        $expr.map_err(|e: anyhow::Error| format!($msg, e))
    }};
}

/// 用户友好错误消息宏。
///
/// 把技术性的内部错误转成用户可理解的中文提示。
/// 适用于日志文件不存在、端口不可用等常见场景。
///
/// # 示例
///
/// ```rust,ignore
/// user_friendly_msg!(
///     std::fs::read_to_string(path),
///     "日志文件读取失败：{}（路径可能不存在）"
/// )
/// ```
#[macro_export]
macro_rules! user_friendly_msg {
    ($expr:expr, $fallback:expr) => {{
        match $expr {
            Ok(v) => Ok(v),
            Err(_) => Err($fallback.to_string()),
        }
    }};
    ($expr:expr, $fmt:literal, $($arg:tt)+) => {{
        match $expr {
            Ok(v) => Ok(v),
            Err(_) => Err(format!($fmt, $($arg)+)),
        }
    }};
}

/// 把错误转换为带上下文的友好消息。
///
/// 返回一个包含原始错误信息的字符串，可用于前端展示。
pub fn to_user_message(e: impl std::fmt::Display, context: &str) -> String {
    format!("{}：{}", context, e)
}

/// 检查路径错误并返回友好的用户消息。
///
/// 根据具体的 `std::io::ErrorKind` 返回不同的用户提示：
/// - `NotFound` → "路径不存在，请确认目标路径正确"
/// - `PermissionDenied` → "没有权限访问该路径，请检查文件权限"
/// - 其他 → "操作失败：{error}"
pub fn io_error_to_user_message(e: &std::io::Error, path: &str) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            format!("路径不存在：{}，请确认路径正确", path)
        }
        std::io::ErrorKind::PermissionDenied => {
            format!("没有权限访问：{}，请检查文件权限", path)
        }
        std::io::ErrorKind::AlreadyExists => {
            format!("路径已存在：{}", path)
        }
        _ => format!("操作失败：{}（{}）", path, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_user_message_includes_context() {
        let msg = to_user_message("connection refused", "网络请求");
        assert!(msg.contains("网络请求"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn io_error_not_found_message() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let msg = io_error_to_user_message(&e, "/tmp/test.txt");
        assert!(msg.contains("路径不存在"));
        assert!(msg.contains("/tmp/test.txt"));
    }

    #[test]
    fn io_error_permission_message() {
        let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let msg = io_error_to_user_message(&e, "/root/secret");
        assert!(msg.contains("没有权限"));
    }

    #[test]
    fn io_error_already_exists_message() {
        let e = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "file exists");
        let msg = io_error_to_user_message(&e, "/tmp/existing.txt");
        assert!(msg.contains("路径已存在"));
    }
}
