//! 统一网络客户端模块。
//!
//! 为所有外部 HTTP 请求（更新 Manifest / HF API / GitHub Release）提供：
//! - 内置代理自动继承（读取系统代理 + 环境变量）
//! - 统一超时策略（连接 15s，读写 60s）
//! - 连接池 + TLS 证书默认校验
//! - 可插拔重试策略（指数退避 + jitter）
//! - 流式下载 API（避免大文件 OOM）
//!
//! # 使用示例
//!
//! ```ignore
//! let client = NetClient::new()?;
//! let body = client.get("https://example.com", &[]).await?;
//! ```

mod client;
mod retry;

pub use client::{NetClient, NetClientBuilder, NetError};
