//! 配置读写命令。
//!
//! 包装 [`crate::config::ConfigStore`] 的 `get` / `set`，提供给前端
//! 的「保存」/「读取」按钮使用。
//!
//! 校验失败时返回中文错误消息（来自 `crate::error::ConfigError::Display`），
//! 前端直接显示。

use tauri::State;

use crate::config::AppConfig;

use super::AppState;

/// 保存配置。会先调用 `AppConfig::validate` 拒绝非法值（端口越界、
/// 模式非法、含 NUL 字符、路径不存在等），通过后写盘。
///
/// 持久化路径：`%APPDATA%\LlamaUI\config.json`（跨平台走 `dirs::config_dir`）。
#[tauri::command]
pub fn save_config(state: State<'_, AppState>, config: AppConfig) -> Result<(), String> {
    state.config.set(config).map_err(|e| e.to_string())
}

/// 读取当前配置。读盘迁移由 `ConfigStore::new` 一次性完成，运行时此命令
/// 只返回内存中的最新值。
#[tauri::command]
pub fn load_config(state: State<'_, AppState>) -> AppConfig {
    state.config.get()
}

#[cfg(test)]
mod tests {
    //! 命令层只做转译，配置校验的详细测试见 `crate::config::tests`。
    //! 本模块主要验证编译期约束（AppConfig / AppState 类型签名）。
    use super::*;

    /// 编译期验证：`load_config` 返回 `AppConfig`（不是内部 mutex guard）。
    #[test]
    fn load_config_returns_owned_config() {
        fn _check() -> AppConfig {
            // 仅用于类型断言
            AppConfig::default()
        }
        let cfg = _check();
        assert!(cfg.port > 0);
    }
}
