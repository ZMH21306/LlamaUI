//! 自动更新检查命令。

use crate::update::{check_for_updates, cleanup_old_installation, UpdateCheckResult};

/// 检查更新（异步，不阻塞事件循环）
#[tauri::command]
pub async fn check_updates() -> Result<UpdateCheckResult, String> {
    tracing::info!(target: "UpdateCmd", "收到检查更新请求");
    // 在 spawn_blocking 中执行同步的检查逻辑，避免阻塞 Tauri 事件循环
    tokio::task::spawn_blocking(move || {
        check_for_updates()
            .map_err(|e| {
                tracing::error!(target: "UpdateCmd", error = %e, "检查更新失败");
                format!("检查更新失败：{}", e)
            })
    })
    .await
    .map_err(|e| format!("检查更新任务失败：{}", e))?
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
            platform: "windows-x64".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: UpdateCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.update_available, true);
        assert_eq!(back.latest_version, "v0.4.0");
    }
}
