//! 远程服务器管理模块。
//!
//! 支持连接远程 llama-server 实例（通过 REST API），与本地服务器形成互补。
//!
//! - [`manager`]：`RemoteServerInfo` / `RemoteServerManager` / `probe_remote_server`

pub mod manager;

pub use manager::{probe_remote_server, RemoteServerInfo, RemoteServerManager};