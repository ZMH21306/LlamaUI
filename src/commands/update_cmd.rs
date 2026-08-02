//! 自动更新检查命令。

use crate::update_check::{check_for_updates, cleanup_old_installation, UpdateCheckResult};

/// 检查更新
#[tauri::command]
pub fn check_updates() -> Result<UpdateCheckResult, String> {
    check_for_updates()
        .map_err(|e| format!("检查更新失败：{}", e))
}

/// 清理旧版本
#[tauri::command]
pub fn cleanup_old_version(path: String) -> Result<(), String> {
    cleanup_old_installation(&path)
        .map_err(|e| format!("清理失败：{}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_check_result_roundtrip() {
        let result = UpdateCheckResult {
            update_available: true,
            latest_version: "v0.4.0".to_string(),
            current_version: "v0.3.0".to_string(),
            download_url: "https://github.com/ZMH21306/LlamaUI/releases/tag/v0.4.0".to_string(),
            release_notes: "New features".to_string(),
            old_installations: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: UpdateCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.update_available, true);
        assert_eq!(back.latest_version, "v0.4.0");
    }
}
