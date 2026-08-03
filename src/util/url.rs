//! URL scheme 白名单校验（纯函数）。
//!
//! 提供可在任何上下文（包括单测）直接调用的 [`validate_url`]。把原本内联
//! 在 `commands::system_cmd::open_external_url` 命令体里的判断逻辑抽到
//! 这里，使业务命令与「白名单 + 长度上限」解耦，未来可被其他模块（如
//! 前端回调 / IPC schema 校验）复用。
//!
//! # 安全目标
//!
//! 阻止以下危险 scheme 通过 `tauri_plugin_opener::open_path` 被
//! `ShellExecuteW` 解析为本地程序：
//! - `file://` —— 本地任意文件读 / 协议执行
//! - `cmd:` / `powershell:` —— 任意命令执行
//! - `javascript:` / `data:` / `vbscript:` —— 浏览器协议注入
//!
//! 仅允许 `http://` 与 `https://`，且长度不超过 [`MAX_URL_BYTES`]。

/// URL 长度上限（字节）。防止异常大的 payload 攻击 `ShellExecuteW` / 前端解析器。
pub const MAX_URL_BYTES: usize = 2048;

#[derive(Debug, PartialEq, Eq)]
pub enum UrlError {
    /// 空字符串或纯空白
    Empty,
    /// 协议不在白名单（只允许 http / https）
    SchemeNotAllowed,
    /// 长度超过 [`MAX_URL_BYTES`]
    TooLong,
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("URL 为空"),
            Self::SchemeNotAllowed => {
                f.write_str("URL scheme 不被允许：仅支持 http:// 与 https://")
            }
            Self::TooLong => f.write_str("URL 长度超过 2048 字节"),
        }
    }
}

impl std::error::Error for UrlError {}

/// 校验一个 URL 字符串是否可被前端 / `open_path` 接受。
///
/// 规则：
/// 1. 去除首尾空白后不能为空；
/// 2. 协议（**不区分大小写**）必须是 `http://` 或 `https://`；
/// 3. 整体长度（不去空白）不超过 [`MAX_URL_BYTES`]。
///
/// 返回校验后的归一化字符串（已 `trim`）便于调用方直接使用，避免重复处理。
///
/// # Examples
///
/// ```
/// // 使用完整路径（模块私有，doctest 需通过 crate 根访问）
/// // assert!(llama_ui_lib::util::url::validate_url("https://example.com").is_ok());
/// ```
pub fn validate_url(input: &str) -> Result<&str, UrlError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(UrlError::Empty);
    }
    // 长度检查用原字符串（保留尾随空白用于审计）
    if input.len() > MAX_URL_BYTES {
        return Err(UrlError::TooLong);
    }
    // 协议判断不区分大小写（但保留原始大小写以兼容大小写敏感的代理 / WAF）
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(UrlError::SchemeNotAllowed);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    //! 全部使用 `expect` / `assert!` 而非 `unwrap` / `unwrap_err`，与项目其他
    //! 测试模块（detect_cmd / job / events）保持一致风格。
    //! 测试 panic 消息前缀 `[URL_TESTS]` 便于在并发测试日志中快速定位。

    use super::*;

    const PREFIX: &str = "[URL_TESTS]";

    /// happy path：http
    #[test]
    fn accept_http_localhost() {
        assert_eq!(
            validate_url("http://127.0.0.1:10897/")
                .expect(&format!("{}: http 应被接受", PREFIX)),
            "http://127.0.0.1:10897/"
        );
    }

    /// happy path：https
    #[test]
    fn accept_https_with_path() {
        assert_eq!(
            validate_url("https://example.com/path")
                .expect(&format!("{}: https 应被接受", PREFIX)),
            "https://example.com/path"
        );
    }

    /// 协议不区分大小写
    #[test]
    fn accept_mixed_case_scheme() {
        assert_eq!(
            validate_url("HTTPS://Example.COM")
                .expect(&format!("{}: 大写 https 应被接受", PREFIX)),
            "HTTPS://Example.COM"
        );
    }

    /// 首尾空白被 trim，但返回值不含空白
    #[test]
    fn trim_whitespace() {
        assert_eq!(
            validate_url("  http://localhost  ")
                .expect(&format!("{}: 含空白 http 应被接受", PREFIX)),
            "http://localhost"
        );
    }

    /// 拒绝 `file://`（防本地文件读取）
    #[test]
    fn reject_file_scheme() {
        let r = validate_url("file:///c:/windows/system32/cmd.exe");
        assert!(r.is_err(), "{}: file:// 必须被拒绝，实际 {:?}", PREFIX, r);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::SchemeNotAllowed
        );
    }

    /// 拒绝 `cmd:`（防任意命令执行）
    #[test]
    fn reject_cmd_scheme() {
        let r = validate_url("cmd:/c calc");
        assert!(r.is_err(), "{}: cmd: 必须被拒绝，实际 {:?}", PREFIX, r);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::SchemeNotAllowed
        );
    }

    /// 拒绝 `javascript:`（浏览器协议注入）
    #[test]
    fn reject_javascript_scheme() {
        let r = validate_url("javascript:alert(1)");
        assert!(r.is_err(), "{}: javascript: 必须被拒绝，实际 {:?}", PREFIX, r);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::SchemeNotAllowed
        );
    }

    /// 拒绝 `data:`（防伪 URL payload）
    #[test]
    fn reject_data_scheme() {
        let r = validate_url("data:text/html,<script>alert(1)</script>");
        assert!(r.is_err(), "{}: data: 必须被拒绝，实际 {:?}", PREFIX, r);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::SchemeNotAllowed
        );
    }

    /// 拒绝空字符串
    #[test]
    fn reject_empty() {
        let r = validate_url("");
        assert!(r.is_err(), "{}: 空字符串必须被拒绝", PREFIX);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::Empty
        );
    }

    /// 拒绝纯空白
    #[test]
    fn reject_whitespace_only() {
        let r = validate_url("    ");
        assert!(r.is_err(), "{}: 纯空白必须被拒绝", PREFIX);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::Empty
        );
    }

    /// 长度超限：3000 字节（实际是 3014）
    #[test]
    fn reject_oversize() {
        let big = format!("http://localhost/{}", "a".repeat(3000));
        let r = validate_url(&big);
        assert!(r.is_err(), "{}: 超长 URL 必须被拒绝", PREFIX);
        assert_eq!(
            r.expect_err(&format!("{}: 已知错误", PREFIX)),
            UrlError::TooLong
        );
    }

    /// 长度刚好等于上限（应通过）
    #[test]
    fn accept_at_max_length() {
        let s = format!(
            "http://localhost/{}",
            "a".repeat(MAX_URL_BYTES - "http://localhost/".len())
        );
        assert_eq!(s.len(), MAX_URL_BYTES);
        assert!(validate_url(&s).is_ok(), "{}: 长度等于上限应被接受", PREFIX);
    }

    /// 大小写不敏感：HTTP / Http / hTTp 都应通过
    #[test]
    fn accept_scheme_case_insensitive() {
        for scheme in &["HTTP://x", "Http://x", "hTTp://x", "HtTpS://x"] {
            assert!(
                validate_url(scheme).is_ok(),
                "{}: scheme `{}` 必须被接受",
                PREFIX,
                scheme
            );
        }
    }

    /// `UrlError` 的 `Display` 文本稳定（前端展示用）
    #[test]
    fn display_text_is_stable() {
        assert_eq!(UrlError::Empty.to_string(), "URL 为空");
        assert_eq!(
            UrlError::SchemeNotAllowed.to_string(),
            "URL scheme 不被允许：仅支持 http:// 与 https://"
        );
        assert_eq!(UrlError::TooLong.to_string(), "URL 长度超过 2048 字节");
    }
}
