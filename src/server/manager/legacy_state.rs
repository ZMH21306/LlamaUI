//! 保留的旧版状态模块
//!
//! 该模块向后兼容旧的 `src/server/state.rs` 和 `src/server/legacy/state.rs`。
//! 新代码应使用 `crate::server::manager::state_store`。

pub use crate::server::legacy::state::{ServerInner, ServerProcess};