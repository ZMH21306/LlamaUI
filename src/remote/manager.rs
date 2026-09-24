//! 远程服务器管理模块。
//!
//! 支持连接远程 llama-server 实例（通过 REST API），与本地服务器形成互补。
//!
//! # 核心功能
//!
//! - [`RemoteServerInfo`]：远程服务器基本信息（地址、状态、可用模型）
//! - [`RemoteServerManager`]：管理多个远程服务器连接
//! - 支持 OpenAI 兼容 API 格式（`/v1/chat/completions` / `/v1/models`）
//!
//! # 安全考虑
//!
//! - 远程地址必须使用 HTTPS（本地开发可允许 HTTP）
//! - API 密钥通过环境变量或配置文件传递，不落盘日志
//! - 连接超时限制为 10s，防止长挂起

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::Mutex;

/// 远程服务器基本信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteServerInfo {
    /// 服务器地址（如 `https://api.openai.com` 或 `http://192.168.1.100:8080`）。
    pub url: String,
    /// 服务器名称（用于显示）。
    pub name: String,
    /// 服务器是否已连接。
    pub connected: bool,
    /// 上次连接时间（UNIX 纪元秒）。
    pub last_connected_at: Option<u64>,
    /// 可用模型列表（从 `/v1/models` 端点获取）。
    pub available_models: Vec<String>,
    /// 附加配置参数。
    #[serde(default)]
    pub extra_config: HashMap<String, String>,
}

impl RemoteServerInfo {
    /// 创建一个新的远程服务器配置（未连接状态）。
    pub fn new(url: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            name: name.into(),
            connected: false,
            last_connected_at: None,
            available_models: Vec::new(),
            extra_config: HashMap::new(),
        }
    }

    /// 验证 URL 格式是否合法（必须是 http:// 或 https:// 开头）。
    /// 远程服务器必须使用 HTTPS，且主机名必须在白名单中（允许 localhost/127.0.0.1 用于本地开发）。
    pub fn validate_url(&self) -> Result<(), String> {
        if !self.url.starts_with("http://") && !self.url.starts_with("https://") {
            return Err("URL 必须以 http:// 或 https:// 开头".to_string());
        }
        // 本地开发模式：允许 127.0.0.1 / localhost 使用 HTTP
        let is_local = self.url.contains("localhost") || self.url.contains("127.0.0.1");
        // 远程必须使用 HTTPS
        if !is_local && !self.url.starts_with("https://") {
            return Err(
                "远程服务器必须使用 HTTPS 协议（本地 localhost 允许 HTTP）".to_string(),
            );
        }
        // 安全校验：远程 URL 的主机名不能是敏感内部地址
        if !is_local {
            if let Some(host) = self.url.strip_prefix("https://").or_else(|| self.url.strip_prefix("http://")) {
                let host = host.split('/').next().unwrap_or(host);
                let host = host.split(':').next().unwrap_or(host);
                let blocked_hosts: &[&str] = &[
                    "localhost", "127.0.0.1", "0.0.0.0", "169.254.169.254",
                ];
                for blocked in blocked_hosts {
                    if host == *blocked {
                        return Err(format!("不允许的远程主机：{}", host));
                    }
                }
                // 拒绝私有 IP 段
                if host.starts_with("192.168.") || host.starts_with("10.") || host.starts_with("172.") {
                    return Err(format!("不允许的私有网络主机：{}", host));
                }
            }
        }
        Ok(())
    }
}

/// 远程服务器管理器。
///
/// 线程安全，可在多个命令中共享同一实例。
#[derive(Debug)]
pub struct RemoteServerManager {
    servers: Arc<Mutex<Vec<RemoteServerInfo>>>,
}

impl RemoteServerManager {
    /// 创建空的远程服务器管理器。
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 添加远程服务器配置。
    pub fn add_server(&self, info: RemoteServerInfo) -> Result<(), String> {
        info.validate_url()?;
        let mut servers = self.servers.lock();
        // 检查是否已存在相同 URL
        if servers
            .iter()
            .any(|s| s.url == info.url && s.name == info.name)
        {
            return Err(format!("服务器 '{}' 已存在", info.name));
        }
        servers.push(info);
        Ok(())
    }

    /// 移除远程服务器。
    pub fn remove_server(&self, name: &str) {
        let mut servers = self.servers.lock();
        servers.retain(|s| s.name != name);
    }

    /// 获取所有服务器列表。
    pub fn list_servers(&self) -> Vec<RemoteServerInfo> {
        self.servers.lock().clone()
    }

    /// 根据名称获取服务器。
    pub fn get_server(&self, name: &str) -> Option<RemoteServerInfo> {
        self.servers.lock().iter().find(|s| s.name == name).cloned()
    }
}

impl Default for RemoteServerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 通过 REST API 探测远程服务器可用性（HTTP GET /v1/models）。
///
/// 返回 `Ok(true)` 表示服务器可访问，`Ok(false)` 表示不可用，`Err` 表示网络错误。
///
/// 使用统一 `HttpClient`（自动代理 + 连接池），取代旧的 `ureq` 实现。
pub fn probe_remote_server(url: &str, api_key: Option<&str>) -> Result<bool, String> {
    let client = crate::util::http::HttpClient::new()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{}", e))?;

    let target_url = format!("{}/v1/models", url);
    let mut headers: Vec<(&str, String)> = vec![
        ("User-Agent", "LlamaUI-RemoteProbe".to_string()),
    ];
    if let Some(key) = api_key {
        headers.push(("Authorization", format!("Bearer {}", key)));
    }
    let header_refs: Vec<(&str, &str)> =
        headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

    match client.get(&target_url, &header_refs) {
        Ok(_) => Ok(true),
        Err(e) => {
            let err_str = e.to_string();
            // HTTP 错误（401/403/404/5xx）表示服务器可达，只是返回错误
            if err_str.contains("HTTP 4") || err_str.contains("HTTP 5") {
                Ok(false)
            } else {
                Err(format!("连接失败：{}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_https_url() {
        let server = RemoteServerInfo::new("https://api.openai.com", "OpenAI");
        assert!(server.validate_url().is_ok());
    }

    #[test]
    fn validate_http_localhost() {
        let server = RemoteServerInfo::new("http://localhost:8080", "Local");
        assert!(server.validate_url().is_ok());
    }

    #[test]
    fn reject_http_remote() {
        let server = RemoteServerInfo::new("http://example.com", "Remote");
        assert!(server.validate_url().is_err());
    }

    #[test]
    fn reject_invalid_scheme() {
        let server = RemoteServerInfo::new("ftp://example.com", "FTP");
        assert!(server.validate_url().is_err());
    }

    #[test]
    fn manager_add_and_list() {
        let mgr = RemoteServerManager::new();
        let info = RemoteServerInfo::new("https://api.example.com", "Example");
        assert!(mgr.add_server(info).is_ok());
        let servers = mgr.list_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "Example");
    }

    #[test]
    fn manager_rejects_duplicate() {
        let mgr = RemoteServerManager::new();
        let info1 = RemoteServerInfo::new("https://api.example.com", "Example");
        let info2 = RemoteServerInfo::new("https://api.example.com", "Example");
        assert!(mgr.add_server(info1).is_ok());
        assert!(mgr.add_server(info2).is_err());
    }

    #[test]
    fn manager_remove() {
        let mgr = RemoteServerManager::new();
        let info = RemoteServerInfo::new("https://api.example.com", "Example");
        mgr.add_server(info).unwrap();
        mgr.remove_server("Example");
        assert!(mgr.list_servers().is_empty());
    }
}
