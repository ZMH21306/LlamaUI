//! 配置层：应用配置结构、存储与校验。
//!
//! 合并原 `config.rs`（`AppConfig` + `ConfigStore`）与 `config_io.rs`（导入导出）。
//!
//! - [`store`]：`AppConfig` 结构体 + `ConfigStore` 单例 + `validate()`
//! - [`io`]  ：`export_config` / `import_config`（JSON 序列化与反序列校验）

pub mod io;
pub mod store;

// 便于外部 `use crate::config::AppConfig` 直接访问
pub use store::{
    AppConfig, ConfigStore, CURRENT_CONFIG_VERSION, DEFAULT_PRO_CUSTOM_COMMAND,
};
pub use io::{export_config, import_config};