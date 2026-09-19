//! 多模型管理模块。
//!
//! 支持多个 .gguf 模型的管理、切换和快速启动。
//!
//! - [`manager`]：`ModelInfo` / `ModelCatalog` / `ModelManager` / `ModelSelector`

pub mod manager;

pub use manager::{ModelCatalog, ModelInfo, ModelManager};