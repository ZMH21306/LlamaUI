//! 业务服务层。
//!
//! 本模块提供跨领域共享的服务：
//! - [`http_client`]：统一 HTTP 客户端（同步 + 异步），自动代理、重试、指数退避
//! - [`validation`]：输入验证服务（model_id、URL、文件名）
//! - [`progress`]：下载进度管理服务（统一事件格式、取消机制）
//!
//! 所有服务都遵循以下原则：
//! 1. **无 Tauri 耦合**：服务本身不直接调用 `app.emit()`，而是通过回调或事件发送器抽象
//! 2. **可测试**：核心逻辑为纯函数，便于单元测试
//! 3. **统一错误**：所有错误通过 `anyhow::Error` 上报，便于 `?` 传播

pub mod http_client;
pub mod progress;
pub mod validation;

pub use http_client::{AsyncHttpClient, HttpClient, HttpResponse};
pub use progress::{DownloadProgress, ProgressCallback, ProgressService};
pub use validation::{ValidationService, ValidationError};