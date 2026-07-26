//! 杂项命令。
//!
//! 不属于「服务控制 / 配置 / 检测 / 初始化」四类核心流程的命令都集中在这里。
//! 当前包含：
//! - `open_external_url`：用系统默认浏览器打开 URL
//!
//! 后续要新增例如「复制到剪贴板」「读取系统信息」等小命令时，也归入此模块。

use tauri::AppHandle;

use crate::util::url::validate_url;

/// 用系统默认浏览器打开 URL（用于 webview 加载不出时给用户提供备选）。
///
/// 安全：URL 校验逻辑委托给 [`crate::util::url::validate_url`] 纯函数，
/// 阻止 `file://` / `cmd:` 等危险 scheme 被 `ShellExecuteW` 执行（防任意协议 RCE）。
#[tauri::command]
pub async fn open_external_url(app: AppHandle, url: String) -> Result<(), String> {
    // 1) 纯函数校验：scheme 白名单 + 长度上限
    let safe_url = validate_url(&url).map_err(|e| e.to_string())?;

    // 2) 调用系统 opener
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(safe_url.to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    //! URL scheme 白名单的端到端测试。
    //!
    //! 直接复用 [`crate::util::url::validate_url`] 的全套单测（11 个 case），
    //! 本模块只保留与命令函数相关的"应当能解析"快速冒烟测试，避免重复。

    use super::*;

    /// 命令函数对空字符串应当返回错误（不会触发 opener 调用）
    #[test]
    fn command_rejects_empty_string() {
        // 通过直接调用 validate_url 模拟命令函数的第一道防线
        assert!(validate_url("").is_err());
    }

    /// 命令函数对 `file://` 必须拒绝（防止本地文件读取）
    #[test]
    fn command_rejects_file_scheme() {
        assert!(validate_url("file:///c:/windows").is_err());
    }
}
