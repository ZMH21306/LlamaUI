//! 配置备份与恢复命令。

use tauri::State;
use crate::backup;
use super::AppState;
use serde::{Deserialize, Serialize};

/// 备份元数据（前端可见的简化版本）
#[derive(Debug, Serialize, Deserialize)]
pub struct BackupMetaResponse {
    pub filename: String,
    pub timestamp: u64,
    pub config_version: u32,
}

/// 备份响应
#[derive(Debug, Serialize)]
pub struct BackupResponse {
    pub filename: String,
    pub backups: Vec<BackupMetaResponse>,
}

/// 创建配置备份
#[tauri::command]
pub fn create_config_backup(
    state: State<'_, AppState>,
) -> Result<BackupResponse, String> {
    let cfg = state.config.get();
    let filename = backup::create_backup(&cfg)
        .map_err(|e| format!("创建备份失败：{}", e))?;
    
    let backups = backup::list_backups()
        .into_iter()
        .map(|b| BackupMetaResponse {
            filename: b.filename,
            timestamp: b.timestamp,
            config_version: b.config_version,
        })
        .collect();
    
    Ok(BackupResponse { filename, backups })
}

/// 列出所有备份
#[tauri::command]
pub fn list_config_backups() -> Result<Vec<BackupMetaResponse>, String> {
    let backups = backup::list_backups()
        .into_iter()
        .map(|b| BackupMetaResponse {
            filename: b.filename,
            timestamp: b.timestamp,
            config_version: b.config_version,
        })
        .collect();
    Ok(backups)
}

/// 恢复配置备份
#[tauri::command]
pub fn restore_config_backup(
    state: State<'_, AppState>,
    filename: String,
) -> Result<(), String> {
    let cfg = backup::restore_backup(&filename)
        .map_err(|e| format!("恢复备份失败：{}", e))?;
    
    state.config.set(cfg)
        .map_err(|e| format!("保存配置失败：{}", e))?;
    
    Ok(())
}

/// 删除配置备份
#[tauri::command]
pub fn delete_config_backup(
    filename: String,
) -> Result<(), String> {
    backup::delete_backup(&filename)
        .map_err(|e| format!("删除备份失败：{}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_meta_response_serialization() {
        let meta = BackupMetaResponse {
            filename: "config-123.json".to_string(),
            timestamp: 123,
            config_version: 1,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"filename\":\"config-123.json\""));
    }
}
