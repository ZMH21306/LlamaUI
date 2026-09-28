//! 下载模块：llama-server 二进制下载 + HuggingFace 模型下载。
//!
//! # 模块结构（按职责划分）
//!
//! | 模块 | 职责 |
//! |------|------|
//! | [`llama_downloader`] | 下载编排层：版本获取、资产匹配、下载调度、SHA256 校验、解压、原子安装 |
//! | [`installer`] | 原子安装器：备份 / 原子替换 / 回滚 / 崩溃恢复 |
//! | [`platform`] | 平台与 GPU 后端检测（OS / CPU 架构 / 可用后端列表） |
//! | [`version`] | llama.cpp 版本解析与比较（`bNNNNN` 构建号 / 语义版本） |
//! | [`hf_downloader`] | HuggingFace Hub 流式下载模型文件 |
//! | [`retry`] | 指数退避重试工具（可用环境变量调参） |
//! | [`mirror`] | 镜像/代理支持（GitHub CDN fallback） |
//!
//! # 依赖方向
//!
//! ```text
//! llama_downloader  ──▶ platform  （资产名推导依赖 OS/架构）
//!        │
//!        ├─────────▶ version    （构建号解析）
//!        ├─────────▶ mirror     （镜像 URL 降级）
//!        ├─────────▶ installer  （原子安装）
//!        └─────────▶ retry      （退避重试）
//!
//! hf_downloader     （独立，仅依赖 reqwest）
//! ```
//!
//! # 扩展新下载源
//!
//! 1. 在本文件注册新子模块；
//! 2. 新模块**不得**反向依赖 `llama_downloader`，保持依赖单向；
//! 3. 平台相关的资产名推导统一走 [`platform`]，不要在下载层重复 `cfg!` 判断。

pub mod hf_downloader;
pub mod installer;
pub mod llama_downloader;
pub mod mirror;
pub mod platform;
pub mod retry;
pub mod version;
