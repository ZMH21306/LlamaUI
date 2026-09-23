//! 统一异步 HTTP 客户端。
//!
//! 替代散落的 `reqwest::Client` 直接调用，统一注入代理、超时、重试与错误映射。

use crate::util::proxy;
use crate::net::retry::RetryPolicy;
use reqwest::{Client as AsyncClient, ClientBuilder as AsyncClientBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;
use thiserror::Error;

/// 网络层错误枚举。
#[derive(Debug, Error)]
pub enum NetError {
    #[error("HTTP 请求失败：{0}")]
    Http(#[from] reqwest::Error),
    #[error("HTTP 状态码 {0}")]
    HttpStatus(StatusCode, String),
    #[error("JSON 解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("重试耗尽：{0}")]
    RetryExhausted(String),
}

impl NetError {
    /// 用户可读的中文描述（前端直接展示用）。
    pub fn user_message(&self) -> String {
        match self {
            NetError::Http(e) => format!("网络请求失败：{}", e),
            NetError::HttpStatus(code, _) => format!("HTTP 状态码 {}，请稍后重试", code),
            NetError::Json(e) => format!("数据解析失败：{}", e),
            NetError::RetryExhausted(e) => format!("重试次数耗尽：{}", e),
        }
    }
}

/// 统一异步 HTTP 客户端。
#[derive(Clone)]
pub struct NetClient {
    client: AsyncClient,
    retry: RetryPolicy,
}

impl NetClient {
    pub fn new() -> Result<Self, NetError> {
        Self::builder().build()
    }

    pub fn with_retry(retry: RetryPolicy) -> Result<Self, NetError> {
        Self::builder().retry(retry).build()
    }

    pub fn builder() -> NetClientBuilder {
        NetClientBuilder::default()
    }

    pub async fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<String, NetError> {
        let response = self.send_get(url, headers).await?;
        let body = response.text().await?;
        Ok(body)
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str, headers: &[(&str, &str)]) -> Result<T, NetError> {
        let body = self.get(url, headers).await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn send_get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, NetError> {
        let mut builder = self.client.get(url);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        self.execute(builder).await
    }

        async fn execute(&self, request: reqwest::RequestBuilder) -> Result<Response, NetError> {
        let mut last_err = String::new();
        for attempt in 0..self.retry.max_attempts.max(1) {
            let result = request.try_clone().map(|b| b.build());
            if let Some(req) = result {
                let response = match req {
                    Ok(r) => self.client.execute(r).await,
                    Err(e) => {
                        last_err = e.to_string();
                        continue;
                    }
                };
                match response {
                    Ok(response) => {
                        let status = response.status();
                        if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
                            return Ok(response);
                        }
                        if status == StatusCode::TOO_MANY_REQUESTS
                            || status == StatusCode::SERVICE_UNAVAILABLE
                            || status.as_u16() >= 500
                        {
                            last_err = format!("HTTP {}，已重试 {} 次", status, attempt + 1);
                            if attempt + 1 < self.retry.max_attempts.max(1) {
                                tokio::time::sleep(self.retry.delay_for_attempt(attempt + 1)).await;
                                continue;
                            }
                        }
                        return Err(NetError::HttpStatus(status, String::new()));
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        if attempt + 1 < self.retry.max_attempts.max(1) {
                            tokio::time::sleep(self.retry.delay_for_attempt(attempt + 1)).await;
                            continue;
                        }
                    }
                }
            }
        }
        Err(NetError::RetryExhausted(last_err))
    }}

/// 构建器。
#[derive(Default)]
pub struct NetClientBuilder {
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    user_agent: Option<String>,
    retry: RetryPolicy,
}

impl NetClientBuilder {
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn build(self) -> Result<NetClient, NetError> {
        let mut builder = AsyncClientBuilder::new()
            .timeout(self.timeout.unwrap_or_else(|| Duration::from_secs(60)))
            .connect_timeout(self.connect_timeout.unwrap_or_else(|| Duration::from_secs(15)))
            .pool_max_idle_per_host(32);

        if let Some(proxy_url) = proxy::read_system_proxy() {
            if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
                builder = builder.proxy(proxy);
                tracing::debug!(target: "NetClient", proxy = %proxy_url, "已注入系统代理");
            }
        }
        if let Some(ua) = self.user_agent {
            builder = builder.user_agent(ua);
        }
        Ok(NetClient {
            client: builder.build()?,
            retry: self.retry,
        })
    }
}
