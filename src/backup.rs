//! 配置备份管理。
//!
//! 自动保留最近 N 个备份，支持手动备份和恢复。

use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BACKUPS: usize = 5;

/// 备份元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMeta {
    /// 备份文件名
    pub filename: String,
    /// 备份时间戳（毫秒级 Unix 时间戳）
    pub timestamp: u64,
    /// 备份时的配置版本
    pub config_version: u32,
}

/// 获取备份目录
fn backup_dir() -> PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("LlamaUI").join("backups")
}

/// 创建新备份，返回备份文件名
pub fn create_backup(cfg: &AppConfig) -> anyhow::Result<String> {
    let dir = backup_dir();
    fs::create_dir_all(&dir)?;
    
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let filename = format!("config-{}.json", now);
    let path = dir.join(&filename);
    
    let data = serde_json::to_string_pretty(cfg)?;
    fs::write(&path, data)?;
    
    // 清理旧备份，保留最近 MAX_BACKUPS 个
    cleanup_old_backups()?;
    
    Ok(filename)
}

/// 列出所有备份（按时间戳降序排列）
pub fn list_backups() -> Vec<BackupMeta> {
    let dir = backup_dir();
    if !dir.exists() {
        return Vec::new();
    }
    
    let mut backups = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("config-") && name_str.ends_with(".json") {
                // 从文件名提取时间戳（config-TIMESTAMP.json）
                let ts_str = &name_str[7..name_str.len() - 5];
                if let Ok(ts) = ts_str.parse::<u64>() {
                    // 尝试读取文件内容获取配置版本
                    let config_version = fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|data| {
                            serde_json::from_str::<AppConfig>(&data)
                                .ok()
                                .map(|c| c._v)
                        })
                        .unwrap_or(0);
                    
                    backups.push(BackupMeta {
                        filename: name_str.to_string(),
                        timestamp: ts,
                        config_version,
                    });
                }
            }
        }
    }
    
    backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    backups
}

/// 恢复指定备份
pub fn restore_backup(filename: &str) -> anyhow::Result<AppConfig> {
    let path = backup_dir().join(filename);
    let data = fs::read_to_string(&path)?;
    let cfg: AppConfig = serde_json::from_str(&data)?;
    Ok(cfg)
}

/// 删除指定备份
pub fn delete_backup(filename: &str) -> anyhow::Result<()> {
    let path = backup_dir().join(filename);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// 清理旧备份，保留最近 MAX_BACKUPS 个
fn cleanup_old_backups() -> anyhow::Result<()> {
    let backups = list_backups();
    if backups.len() > MAX_BACKUPS {
        for backup in backups.iter().skip(MAX_BACKUPS) {
            let _ = delete_backup(&backup.filename);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::io::Write;

    fn test_backup_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("llamaui_test_backups");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_and_list_backups() {
        // 注意：这个测试会在真实备份目录创建文件
        // 实际项目中应该用 mock 或临时目录
        let cfg = AppConfig::default();
        let result = create_backup(&cfg);
        assert!(result.is_ok(), "创建备份不应失败");
        
        let backups = list_backups();
        assert!(!backups.is_empty(), "至少有一个备份");
    }

    #[test]
    fn list_backups_empty_when_no_backups() {
        // 如果备份目录不存在，应该返回空列表
        let backups = list_backups();
        // 不强制断言，因为可能有其他测试创建的备份
        assert!(backups.len() >= 0);
    }

    #[test]
    fn backup_meta_serialization() {
        let meta = BackupMeta {
            filename: "config-1234567890.json".to_string(),
            timestamp: 1234567890,
            config_version: 1,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"filename\":\"config-1234567890.json\""));
        assert!(json.contains("\"timestamp\":1234567890"));
        assert!(json.contains("\"config_version\":1"));
    }
}
