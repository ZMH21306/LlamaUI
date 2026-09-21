//! 统一 HTTP 客户端服务。
//!
//! 所有网络请求（HF API、GitHub API、llama.cpp release）共用此模块：
//! - 从 `util::proxy` 读取系统代理配置
//! - 支持自定义 Header（如 Authorization）
//! - 统一的超时策略（连接 15s，读取 60s）
//! - 提供同步（blocking）和异步两种 API
//!
//! # 设计原则
//!
//! 1. **单例可复用**：`HttpClient` 内部维护连接池，支持 keep-alive
//! 2. **代理自动继承**：调用方无需手动处理代理
//! 3. **统一错误**：所有网络错误通过 anyhow 上报

use std::time::Duration;

use reqwest::{Client as AsyncClient, ClientBuilder as AsyncClientBuilder};
use reqwest::blocking::{Client as SyncClient, ClientBuilder as SyncClientBuilder};

/// 同步 HTTP 客户端（用于 blocking 上下文，如 spawn_blocking）。
///
/// 自动注入系统代理和 TLS 证书验证。
pub struct HttpClient {
    client: SyncClient,
}

impl HttpClient {
    /// 构建默认客户端（无自定义 Header）。
    pub fn new() -> anyhow::Result<Self> {
        let client = Self::builder()?.build()?;
        Ok(Self { client })
    }

    /// 构建带自定义 User-Agent 的客户端。
    pub fn with_user_agent(ua: &str) -> anyhow::Result<Self> {
        let client = Self::builder()?
            .user_agent(ua)
            .build()?;
        Ok(Self { client })
    }

    fn builder() -> anyhow::Result<SyncClientBuilder> {
        let mut builder = SyncClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15));

        if let Some(proxy_url) = crate::util::proxy::read_system_proxy() {
            if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
                builder = builder.proxy(proxy);
                tracing::debug!(target: "HttpClient", proxy = %proxy_url, "已注入系统代理");
            }
        }

        Ok(builder)
    }

    /// 发送 GET 请求并返回响应体字符串。
    ///
    /// 支持自定义 Header（如 `Authorization`）。
    pub fn get(&self, url: &str, extra_headers: &[(&str, &str)]) -> anyhow::Result<String> {
        let mut request = self.client.get(url);
        for (k, v) in extra_headers {
            request = request.header(*k, *v);
        }
        let response = request.send()?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text()?;
            return Err(anyhow::anyhow!("HTTP {}: {}", status, body));
        }
        Ok(response.text()?)
    }

    /// 发送 HEAD 请求并返回 Content-Length（若存在）。
    pub fn head(&self, url: &str) -> anyhow::Result<Option<u64>> {
        let response = self.client.head(url).send()?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!("HTTP {}: HEAD {}", status, url));
        }
        Ok(response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok()))
    }

    /// 发送 HEAD 请求并返回 Content-Length（若存在），支持自定义 Header。
    pub fn head_with_headers(&self, url: &str, extra_headers: &[(&str, &str)]) -> anyhow::Result<Option<u64>> {
        let mut request = self.client.head(url);
        for (k, v) in extra_headers {
            request = request.header(*k, *v);
        }
        let response = request.send()?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!("HTTP {}: HEAD {}", status, url));
        }
        Ok(response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok()))
    }
}

/// 异步 HTTP 客户端（用于 async 上下文）。
///
/// 自动注入系统代理和 TLS 证书验证。
pub struct AsyncHttpClient {
    client: AsyncClient,
}

impl AsyncHttpClient {
    /// 构建默认异步客户端。
    pub fn new() -> anyhow::Result<Self> {
        let client = Self::builder()?.build()?;
        Ok(Self { client })
    }

    /// 构建带自定义 User-Agent 的异步客户端。
    pub fn with_user_agent(ua: &str) -> anyhow::Result<Self> {
        let client = Self::builder()?
            .user_agent(ua)
            .build()?;
        Ok(Self { client })
    }

    fn builder() -> anyhow::Result<AsyncClientBuilder> {
        let mut builder = AsyncClientBuilder::new()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15));

        if let Some(proxy_url) = crate::util::proxy::read_system_proxy() {
            if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
                builder = builder.proxy(proxy);
                tracing::debug!(target: "AsyncHttpClient", proxy = %proxy_url, "已注入系统代理");
            }
        }

        Ok(builder)
    }

    /// 发送 GET 请求并返回响应体字符串。
    pub async fn get(&self, url: &str, extra_headers: &[(&str, &str)]) -> anyhow::Result<String> {
        let mut request = self.client.get(url);
        for (k, v) in extra_headers {
            request = request.header(*k, *v);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(anyhow::anyhow!("HTTP {}: {}", status, body));
        }
        Ok(response.text().await?)
    }

    /// 发送 HEAD 请求并返回 Content-Length（若存在）。
    pub async fn head(&self, url: &str) -> anyhow::Result<Option<u64>> {
        let response = self.client.head(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!("HTTP {}: HEAD {}", status, url));
        }
        Ok(response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_creation() {
        let client = HttpClient::new();
        assert!(client.is_ok());
    }

    #[test]
    fn async_http_client_creation() {
        let client = AsyncHttpClient::new();
        assert!(client.is_ok());
    }
}