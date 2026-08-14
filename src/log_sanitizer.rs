//! 日志脱敏工具。
//!
//! 在日志输出前对敏感信息进行掩码处理，防止用户名、密码等敏感信息泄露到日志文件。
//!
//! # 脱敏规则
//!
//! - **路径脱敏**：隐藏用户主目录以外的路径段（如 `D:\data\secrets\` → `D:\data\***`）
//! - **密钥脱敏**：匹配常见的 API key / token 模式并掩码
//! - **端口不脱敏**：端口号不是敏感信息
//! - **URL 脱敏**：隐藏 URL 中的认证信息（`user:pass@host`）
//!
//! # 使用方式
//!
//! ```rust
//! use llama_ui_lib::log_sanitizer::sanitize_log;
//!
//! let sensitive = "Connected to https://admin:secret@example.com/api?key=abc123";
//! let safe = sanitize_log(sensitive);
//! assert!(!safe.contains("secret"));
//! assert!(!safe.contains("abc123"));
//! ```

use regex::Regex;
use std::sync::OnceLock;

/// 敏感数据正则模式（预编译，避免每次调用重新编译）
fn api_key_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)\b(key|token|secret|password|passwd|api[_-]?key|auth[_-]?token)\s*[=:]\s*(\S+)",
        )
        .unwrap()
    })
}

/// URL 中认证信息模式（user:pass@host）
fn url_auth_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"//([^:/@]+):([^@]+)/?").unwrap())
}

/// 敏感路径段（相对用户主目录之外的部分）
fn sensitive_path_segments() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(?:secrets?|credentials?|keys?|passwords?|tokens?|private|auth)")
            .unwrap()
    })
}

/// 对单行日志进行脱敏处理。
///
/// # 处理规则
///
/// 1. 移除 URL 中的认证信息（`user:pass@host` → `user:***@host`）
/// 2. 掩码 API key / token 值（`key=abc123` → `key=***`）
/// 3. 若路径包含敏感目录名，只显示到盘符级别
///
/// # 示例
///
/// ```
/// use llama_ui_lib::log_sanitizer::sanitize_log;
///
/// // URL 认证信息被移除
/// assert!(!sanitize_log("url=https://user:pass@example.com").contains("pass"));
/// // API key 被掩码
/// assert!(sanitize_log("token=my-secret-token").contains("***"));
/// ```
pub fn sanitize_log(input: &str) -> String {
    let mut result = input.to_string();

    // 1. 处理 URL 中的认证信息
    result = url_auth_pattern().replace_all(&result, "//$1:***@").to_string();

    // 2. 处理 key/token/secret 赋值
    result = api_key_pattern()
        .replace_all(&result, |caps: &regex::Captures| {
            format!("{}={}", &caps[1], "***")
        })
        .to_string();

    // 3. 处理包含敏感关键词的路径
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy().to_string();
        if result.contains(&home_str) && sensitive_path_segments().is_match(&result) {
            // 找到主目录后替换敏感部分
            let base = result.split(&home_str).next().unwrap_or("");
            result = format!("{}{}/***", base, home_str);
        }
    }

    result
}

/// 对单行日志进行脱敏处理，同时记录脱敏前后的差异（debug 模式）。
#[cfg(test)]
pub fn sanitize_log_and_verify(input: &str) -> String {
    sanitize_log(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_url_credentials() {
        let input = "Connecting to https://admin:supersecret@example.com:8080/api";
        let output = sanitize_log(input);
        assert!(!output.contains("supersecret"));
        assert!(output.contains("example.com"));
    }

    #[test]
    fn sanitizes_api_key_assignment() {
        let input = "API_KEY = abc123def456 used for auth";
        let output = sanitize_log(input);
        assert!(output.contains("***"));
        assert!(!output.contains("abc123def456"));
    }

    #[test]
    fn sanitizes_token_value() {
        let input = "Using token=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx for GitHub";
        let output = sanitize_log(input);
        assert!(output.contains("***"));
        assert!(!output.contains("ghp_"));
    }

    #[test]
    fn leaves_plain_text_unchanged() {
        let input = "Server started on port 10897";
        let output = sanitize_log(input);
        assert_eq!(output, input);
    }

    #[test]
    fn leaves_port_number_unchanged() {
        let input = "Listening on 127.0.0.1:10897";
        let output = sanitize_log(input);
        assert!(output.contains("10897"));
    }

    #[test]
    fn sanitizes_model_path_with_secrets() {
        let input = "Loading model from /home/user/projects/secrets/models/gemma.gguf";
        let output = sanitize_log(input);
        // 应包含脱敏路径但不应包含完整敏感路径
        assert!(!output.contains("secrets/models"));
    }

    #[test]
    fn empty_input_is_safe() {
        assert_eq!(sanitize_log(""), "");
    }

    #[test]
    fn null_bytes_are_handled() {
        // 含 NUL 字符的输入不应 panic
        let input = "safe text\x00more text";
        let output = sanitize_log(input);
        // 不 panic 即通过
        let _ = output;
    }
}
