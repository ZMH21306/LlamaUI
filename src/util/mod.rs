//! 通用工具模块。
//!
//! 子模块：
//! - [`path`]：路径标准化、段匹配、白名单校验
//! - [`time`]：时间戳格式化
//! - [`url`]：URL scheme 白名单校验（纯函数）
//! - [`process`]：静默子进程封装（Windows 下隐藏控制台窗口）
//!
//! 设计原则：此模块**不依赖** `crate::config` / `crate::server` / `crate::detect`，
//! 任何层都可以 `use crate::util::*`。

pub mod path;
pub mod process;
pub mod time;
pub mod url;
