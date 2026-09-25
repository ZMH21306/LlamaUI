//! 自动更新检查命令。

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter};

use crate::events::{UpdateDownloadProgress, UpdateState, EVT_UPDATE_DOWNLOAD_PROGRESS, EVT_UPDATE_STATE};
use crate::update::{
    check_for_updates, cleanup_old_installation, download_update, install_update, UpdateCheckResult,
    UPDATE_DOWNLOAD_CANCEL,
};

/// 下载并安装更新（调用后阻塞，直到完成或取消）
#[tauri::command]
pub async fn download_update_cmd(
    app: AppHandle,
) -> Result<(), String> {
    let result = check_for_updates().await.map_err(|e| format!("检查更新失败：{}", e))?;
    if !result.update_available {
        return Ok(());
    }

    // 重置取消标志
    UPDATE_DOWNLOAD_CANCEL.store(false, Ordering::Relaxed);

    // 确定下载目录（缓存目录）
    let download_dir = dirs::cache_dir().unwrap_or_else(|| std::env::temp_dir());
    let dest_path = download_dir.join(format!("LlamaUI-{}-update.zip", result.latest_version));

    // 发送下载开始状态
    let _ = app.emit(EVT_UPDATE_STATE, UpdateState::DownloadStarted {
        total_bytes: result.file_size,
    });

    // 在后台任务中下载
    let app_clone = app.clone();
    let dest_path_clone = dest_path.clone();

    let download_task = tokio::spawn(async move {
        download_update(
            &app_clone,
            &result.download_url,
            &dest_path_clone,
            result.file_size,
        ).await
    });

    // 等待下载完成
    match download_task.await {
        Ok(Ok(download_result)) => {
            let download_path = download_result.download_path;
            let file_size = download_result.file_size;

            // 发送下载完成状态
            let _ = app.emit(EVT_UPDATE_STATE, UpdateState::DownloadCompleted {
                download_path: download_path.clone(),
                file_size,
            });
            let _ = app.emit(
                EVT_UPDATE_DOWNLOAD_PROGRESS,
                UpdateDownloadProgress {
                    stage: "completed".to_string(),
                    progress: 1.0,
                    downloaded: file_size,
                    total: file_size,
                    speed_mbps: 0.0,
                    eta_secs: None,
                    message: "下载完成，开始安装".to_string(),
                },
            );

            // 安装更新（同步执行）
            match install_update(&app, std::path::Path::new(&download_path), file_size).await {
                Ok(_) => {
                    let _ = app.emit(EVT_UPDATE_STATE, UpdateState::Completed {
                        new_version: String::new(),
                    });
                    tracing::info!(target: "UpdateCmd", "更新安装成功");
                }
                Err(e) => {
                    let _ = app.emit(EVT_UPDATE_STATE, UpdateState::Failed {
                        error: format!("安装失败：{}", e),
                    });
                    tracing::error!(target: "UpdateCmd", error = %e, "更新安装失败");
                    return Err(format!("安装失败：{}", e));
                }
            }
        }
        Ok(Err(e)) => {
            // 发送失败状态
            let _ = app.emit(
                EVT_UPDATE_STATE,
                UpdateState::Failed {
                    error: e.to_string(),
                },
            );
            return Err(e.to_string());
        }
        Err(e) => {
            // spawn_blocking 失败
            let _ = app.emit(
                EVT_UPDATE_STATE,
                UpdateState::Failed {
                    error: format!("下载任务执行失败: {}", e),
                },
            );
            return Err(format!("下载任务执行失败: {}", e));
        }
    }

    Ok(())
}

/// 取消当前正在进行的更新下载
#[tauri::command]
pub async fn cancel_update_download(
    app: AppHandle,
) -> Result<(), String> {
    UPDATE_DOWNLOAD_CANCEL.store(true, Ordering::Relaxed);
    let _ = app.emit(EVT_UPDATE_STATE, UpdateState::Cancelled);
    let _ = app.emit(
        EVT_UPDATE_DOWNLOAD_PROGRESS,
        UpdateDownloadProgress {
            stage: "cancelled".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            speed_mbps: 0.0,
            eta_secs: None,
            message: "下载已取消".to_string(),
        },
    );
    Ok(())
}

/// 检查更新（异步，不阻塞事件循环）
#[tauri::command]
pub async fn check_updates() -> Result<UpdateCheckResult, String> {
    tracing::info!(target: "UpdateCmd", "收到检查更新请求");
    check_for_updates()
        .await
        .map_err(|e| {
            tracing::error!(target: "UpdateCmd", error = %e, "检查更新失败");
            format!("检查更新失败：{}", e)
        })
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
            file_size: 1024 * 1024 * 50,
            sha256: None,
            signature_verified: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: UpdateCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.update_available, true);
        assert_eq!(back.latest_version, "v0.4.0");
    }
}
