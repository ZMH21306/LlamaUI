//! 通用工具模块。
//!
//! 子模块：
//! - [`path`]：路径标准化、段匹配、白名单校验
//! - [`time`]：时间戳格式化
//! - [`url`]：URL scheme 白名单校验（纯函数）
//!
//! 设计原则：此模块**不依赖** `crate::config` / `crate::server` / `crate::detect`，
//! 任何层都可以 `use crate::util::*`。

pub mod path;
pub mod time;
pub mod url;
