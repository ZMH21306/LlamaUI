//! 远程服务器管理命令。
//!
//! 提供远程 llama-server 实例的连接管理，支持 OpenAI 兼容 API 格式。

use super::AppState;
use tauri::State;

/// 添加远程服务器配置。
#[tauri::command]
pub fn add_remote_server(
    state: State<'_, AppState>,
    info: crate::remote_server::RemoteServerInfo,
) -> Result<(), String> {
    state.remote_server_manager.add_server(info)
}

/// 移除远程服务器。
#[tauri::command]
pub fn remove_remote_server(state: State<'_, AppState>, name: String) {
    state.remote_server_manager.remove_server(&name);
}

/// 列出所有远程服务器。
#[tauri::command]
pub fn list_remote_servers(state: State<'_, AppState>) -> Vec<crate::remote_server::RemoteServerInfo> {
    state.remote_server_manager.list_servers()
}

/// 获取指定名称的远程服务器。
#[tauri::command]
pub fn get_remote_server(
    state: State<'_, AppState>,
    name: String,
) -> Option<crate::remote_server::RemoteServerInfo> {
    state.remote_server_manager.get_server(&name)
}

/// 探测远程服务器是否可用（异步，返回结果字符串）。
#[tauri::command]
pub async fn probe_remote_server(
    state: State<'_, AppState>,
    url: String,
    api_key: Option<String>,
) -> Result<bool, String> {
    crate::remote_server::probe_remote_server(&url, api_key.as_deref()).await
}
