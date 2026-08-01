//! 配置导入/导出 Tauri 命令。
//!
//! 提供三个命令：
//! - `export_config_json`：返回配置 JSON 字符串（前端用 dialog 选路径后写盘）
//! - `import_config_from_file`：从指定路径读取 JSON 并导入
//! - `export_config_to_file`：将当前配置写入指定路径

use std::path::PathBuf;
use tauri::State;
use crate::config_io;
use super::AppState;

/// 导出配置为 JSON 字符串
#[tauri::command]
pub fn export_config_json(
    state: State<'_, AppState>,
) -> Result<String, String> {
    let cfg = state.config.get();
    config_io::export_config(&cfg)
        .map_err(|e| format!("导出配置失败：{}", e))
}

/// 将当前配置写入指定路径
#[tauri::command]
pub fn export_config_to_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let cfg = state.config.get();
    let json = config_io::export_config(&cfg)
        .map_err(|e| format!("导出配置失败：{}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("写入文件失败：{}", e))
}

/// 从指定路径读取 JSON 文件并导入配置
#[tauri::command]
pub fn import_config_from_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    // 安全校验：防止路径穿越
    let pb = PathBuf::from(&path);
    if !pb.exists() {
        return Err(format!("文件不存在：{}", path));
    }
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取文件失败：{}", e))?;
    let cfg = config_io::import_config(&json)
        .map_err(|e| format!("导入失败：{}", e))?;
    state.config.set(cfg)
        .map_err(|e| format!("保存配置失败：{}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_import_roundtrip() {
        let json = r#"{"_v":1,"models_dir":"m","ctx_size":4096,"n_gpu_layers":0,"flash_attn":false,"mtp":false,"mtp_draft_n_max":3,"port":8080,"auto_port":true,"extra_args":"","mode":"normal","custom_command":""}"#;
        let cfg = config_io::import_config(json).unwrap();
        assert_eq!(cfg._v, 1);
        let exported = config_io::export_config(&cfg).unwrap();
        let restored = config_io::import_config(&exported).unwrap();
        assert_eq!(cfg._v, restored._v);
    }
}
