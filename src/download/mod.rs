//! 下载模块：llama-server 下载 + HF 模型下载。
//!
//! - [`llama_downloader`]：从 GitHub Releases 下载 llama-server 二进制
//! - [`installer`]：原子安装器（备份 / 原子替换 / 回滚 / 崩溃恢复）
//! - [`platform`]：平台与 GPU 后端检测
//! - [`version`]：版本解析与比较
//! - [`hf_downloader`]：从 HuggingFace Hub 流式下载模型文件
//! - [`retry`]：通用重试工具（指数退避）
//! - [`mirror`]：镜像/代理支持（GitHub CDN fallback）

pub mod hf_downloader;
pub mod installer;
pub mod llama_downloader;
pub mod mirror;
pub mod platform;
pub mod retry;
pub mod version;
