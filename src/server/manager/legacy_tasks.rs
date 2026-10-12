//! 保留的旧版任务管理模块
//!
//! 该模块向后兼容旧的任务管理实现。
//! 新代码应使用 `crate::server::manager::task_factory::TaskManager`。

pub use crate::server::manager::task_factory::TaskManager;