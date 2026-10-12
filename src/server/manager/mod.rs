//! 服务器管理模块 - 统一的服务器生命周期和状态管理。
//!
//! 本模块旨在重构原有的server子系统，以解决以下问题：
//! - `lifecycle.rs`中700+行的巨型文件
//! - `state.rs`与`lifecycle.rs`的紧密耦合
//! - `tasks.rs`与`state.rs`的复杂依赖关系
//! - `port.rs`中分散的端口管理逻辑
//! 
//! 新架构采用关注点分离原则：
//! - `state_store.rs`：纯状态存储，读多写少
//! - `lifecycle.rs`：状态转换和任务协调
//! - `task_factory.rs`：后台任务工厂模式
//! - `port_manager.rs`：统一的端口管理
//! 
//! 该模块保持向后兼容性，通过`legacy`子模块重用原有代码，
//! 并提供新的统一API供上层使用。

pub mod state_store;
pub mod legacy_lifecycle;
pub mod legacy_state;
pub mod legacy_tasks;
pub mod legacy_port;
pub mod legacy_winapi;
pub mod legacy_job;
pub mod legacy_metrics;
pub mod legacy_log_channel;
pub mod legacy_log_truncate;
pub mod legacy_cmdline;
pub mod task_factory;
pub mod port_manager;

/// 统一的服务器状态存储 - 提供对服务器状态的统一访问接口
pub use state_store::ServerStateStore;

/// 服务器生命周期管理器 - 协调服务器的启动、停止和状态转换
pub use legacy_lifecycle::ServerLifecycleManager;

/// 后台任务管理器 - 管理所有后台任务的生命周期
pub use legacy_tasks::TaskManager;

/// 端口管理器 - 统一的端口选择和管理
pub use legacy_port::PortManager;