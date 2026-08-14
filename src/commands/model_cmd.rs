//! 多模型管理命令。
//!
//! 提供模型目录扫描、模型列表查询、按标签过滤、快速切换等能力。

use super::AppState;
use tauri::State;

/// 列出所有已扫描的模型。
#[tauri::command]
pub fn list_models(state: State<'_, AppState>) -> Vec<crate::model_management::ModelInfo> {
    state.model_manager.all_models()
}

/// 按标签过滤模型。
#[tauri::command]
pub fn filter_models_by_tag(
    state: State<'_, AppState>,
    tag: String,
) -> Vec<crate::model_management::ModelInfo> {
    state.model_manager.filter_models_by_tag(&tag)
}

/// 刷新所有模型目录索引。
#[tauri::command]
pub fn refresh_models(state: State<'_, AppState>) {
    state.model_manager.refresh_all();
}

/// 选择当前活跃的模型。
#[tauri::command]
pub fn select_model(state: State<'_, AppState>, path: String) -> Result<(), String> {
    state.model_manager.select_model(&path);
    Ok(())
}

/// 获取当前选中的模型路径。
#[tauri::command]
pub fn get_selected_model(state: State<'_, AppState>) -> Option<String> {
    state.model_manager.selected_model()
}
