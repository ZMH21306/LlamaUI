//! HTTP 代理读取工具（跨模块共享）。
//!
//! 提供 [`read_system_proxy()`]：读取 Windows 系统代理配置（兼容 Clash/V2Ray 等透明代理）。
//! 优先读 `HKCU\\...\\ProxyServer`，再兜底环境变量（大小写不敏感）。
//! 返回 `Some("http://host:port")` 或 `None`（无代理/读取失败）。
//!
//! # 设计说明
//!
//! 原实现在 `commands::hf_model_cmd` 中为私有函数，现移至 `util::proxy` 以便
//! `download_engine` 等网络层模块也能复用，确保 API 请求与文件下载请求走同一套代理。

/// 读取 Windows 系统代理配置（兼容 Clash/V2Ray 等透明代理）。
/// 优先读 `HKCU\\...\\ProxyServer`，再兜底环境变量（大小写不敏感）。
/// 返回 `Some("http://host:port")` 或 `None`（无代理/读取失败）。
pub fn read_system_proxy() -> Option<String> {
    // 1) 环境变量（所有平台通用，Clash 等也支持）
    for key in &["ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                tracing::debug!(target: "Proxy", source = "env", key = %key, value = %v, "使用环境变量代理");
                return Some(v);
            }
        }
    }
    // 2) Windows 注册表：系统代理设置
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let settings = match hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings") {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(target: "Proxy", error = %e, "无法打开 Internet Settings 注册表项");
                return None;
            }
        };
        // P0-7 修复：必须同时检查 ProxyEnable。Clash 关闭后 ProxyServer 残留
        // 127.0.0.1:7897 但 ProxyEnable=0，若只读 ProxyServer 会把已失效的代理
        // 注入给 reqwest，导致 HTTPS 请求走明文 CONNECT 失败（SSL UNEXPECTED_EOF），
        // 而直连又没被使用，表现为"浏览器能开 HF、程序报网络错误"。
        let proxy_enabled = match settings.get_value::<u32, _>("ProxyEnable") {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(target: "Proxy", error = %e, "读取 ProxyEnable 失败，默认禁用代理");
                return None;
            }
        };
        if proxy_enabled == 0 {
            tracing::debug!(target: "Proxy", "ProxyEnable=0，不使用代理");
            return None;
        }
        let proxy_server = match settings.get_value::<String, _>("ProxyServer") {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(target: "Proxy", error = %e, "读取 ProxyServer 失败");
                return None;
            }
        };
        if proxy_server.is_empty() {
            tracing::debug!(target: "Proxy", "ProxyServer 为空");
            return None;
        }
        // ProxyServer 格式：`host:port` 或 `http=host:port;https=host:port`
        // Clash 输出通常是 `http=127.0.0.1:7897;https=127.0.0.1:7897`
        // 取第一个匹配的协议，或整体作为 HTTP 代理。
        let mut result = String::new();
        for line in proxy_server.split(';') {
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Some((k, v)) = line.split_once('=') {
                if k.eq_ignore_ascii_case("http") || k.eq_ignore_ascii_case("https") {
                    // Clash 通常输出 http= 形式；若为 https= 则走 HTTPS 代理
                    result = format!("{}://{}", k.to_lowercase(), v.trim());
                    break;
                }
            } else {
                // 纯 host:port 形式（IE 风格），默认 HTTP 代理
                result = format!("http://{}", line);
                break;
            }
        }
        if result.is_empty() {
            tracing::debug!(target: "Proxy", raw = %proxy_server, "无法解析代理服务器地址");
            return None;
        }
        tracing::debug!(target: "Proxy", source = "registry", proxy = %result, "使用注册表代理");
        Some(result)
    }
    #[cfg(not(windows))]
    {
        None
    }
}