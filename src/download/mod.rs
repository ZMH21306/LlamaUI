//! 下载模块：llama-server 下载 + HF 模型下载。
//!
//! - [`llama_downloader`]：从 GitHub Releases 下载 llama-server 二进制
//! - [`hf_downloader`]：从 HuggingFace Hub 流式下载模型文件

pub mod hf_downloader;
pub mod llama_downloader;