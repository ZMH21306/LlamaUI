//! 插件管理命令。
//!
//! 提供插件的注册、卸载、列表查询和初始化能力。

use super::AppState;
use tauri::State;

/// 列出所有已注册插件。
#[tauri::command]
pub fn list_plugins(state: State<'_, AppState>) -> Vec<crate::plugin_framework::PluginMetadata> {
    state.plugin_manager.list_plugins()
}
