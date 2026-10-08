//! 保留的旧版端口管理模块
//!
//! 该模块向后兼容旧的端口管理实现。
//! 新代码应使用 `crate::server::manager::port_manager::PortManager`。

pub use crate::server::manager::port_manager::PortManager;