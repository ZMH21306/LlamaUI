//! 自动更新检查命令。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Emitter};

use crate::events::{
    UpdateDownloadProgress, UpdateState, EVT_UPDATE_DOWNLOAD_PROGRESS, EVT_UPDATE_STATE,
};
use crate::update::{
    check_for_updates, cleanup_old_installation, create_update_download_cancel, install_update,
    remove_update_download_cancel, UpdateCheckResult, UPDATE_DOWNLOAD_CANCELS,
};

/// 发送更新状态事件（辅助函数）
fn emit_update_state(app: &AppHandle, state: UpdateState) {
    let _ = app.emit(EVT_UPDATE_STATE, state);
}

/// 下载并安装更新（调用后阻塞，直到完成或取消）
#[tauri::command]
pub async fn download_update_cmd(app: AppHandle) -> Result<(), String> {
    let start_time = Instant::now();
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    // 发送检查开始状态
    emit_update_state(
        &app,
        UpdateState::Checking {
            current_version: current_version.clone(),
            message: "正在检查更新...".to_string(),
        },
    );

    let check_result = check_for_updates()
        .await
        .map_err(|e| format!("检查更新失败：{}", e))?;

    if !check_result.update_available {
        emit_update_state(
            &app,
            UpdateState::UpToDate {
                current_version: current_version.clone(),
                message: format!("当前版本已是最新 {}", current_version),
            },
        );
        return Ok(());
    }

    // 发送有新版本状态
    emit_update_state(
        &app,
        UpdateState::Available {
            latest_version: check_result.latest_version.clone(),
            current_version: current_version.clone(),
            release_notes: check_result.release_notes.clone(),
            download_url: check_result.download_url.clone(),
            file_size: check_result.file_size,
            sha256: check_result.sha256.clone(),
            message: format!(
                "发现新版本 {} (当前 {})，大小 {:.1}MB，发布说明: {}",
                check_result.latest_version,
                current_version,
                check_result.file_size as f64 / 1_048_576.0,
                check_result.release_notes
            ),
        },
    );

    // 重置取消标志（兼容旧代码）
    crate::update::UPDATE_DOWNLOAD_CANCEL.store(false, Ordering::Relaxed);

    // P0-9: 为每个下载任务创建独立的取消信号
    let (download_id, cancel_rx) = create_update_download_cancel();
    let download_id_for_task = download_id.clone();

    // 确定下载目录（缓存目录）
    let download_dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let dest_path = download_dir.join(format!(
        "LlamaUI-{}-update.zip",
        check_result.latest_version
    ));

    // 发送下载开始状态
    emit_update_state(
        &app,
        UpdateState::DownloadStarted {
            total_bytes: check_result.file_size,
            version: check_result.latest_version.clone(),
            message: format!(
                "开始下载版本 {}，大小 {:.1}MB",
                check_result.latest_version,
                check_result.file_size as f64 / 1_048_576.0
            ),
        },
    );

    // 在后台任务中下载（带重试机制）
    let download_result = download_with_retry(
        &app,
        &check_result.download_url,
        &dest_path,
        check_result.file_size,
        cancel_rx,
        &check_result.latest_version,
        3, // 最大重试次数
    )
    .await;

    // 等待下载完成
    match download_result {
        Ok(download_result) => {
            remove_update_download_cancel(&download_id_for_task);
            let download_path = download_result.download_path;
            let file_size = download_result.file_size;

            // 发送下载完成状态
            emit_update_state(
                &app,
                UpdateState::DownloadCompleted {
                    download_path: download_path.clone(),
                    file_size,
                    version: check_result.latest_version.clone(),
                    message: format!("下载完成，准备安装版本 {}", check_result.latest_version),
                },
            );

            // 验证下载文件
            let verification_result = verify_download_file(
                &download_path,
                file_size,
                &check_result.sha256,
                &app,
                &check_result.latest_version,
            );

            if let Err(e) = verification_result {
                emit_update_state(
                    &app,
                    UpdateState::Failed {
                        error: format!("文件验证失败: {}", e),
                        version: check_result.latest_version.clone(),
                        stage: "verification".to_string(),
                        message: format!("文件验证失败: {}", e),
                    },
                );
                return Err(format!("文件验证失败: {}", e));
            }

            // 安装更新（同步执行）
            match install_update(
                &app,
                std::path::Path::new(&download_path),
                file_size,
                &check_result.latest_version,
            )
            .await
            {
                Ok(_) => {
                    let elapsed_ms = start_time.elapsed().as_millis() as u64;
                    emit_update_state(
                        &app,
                        UpdateState::Completed {
                            new_version: check_result.latest_version.clone(),
                            elapsed_ms,
                            message: format!(
                                "更新成功！已安装 {}，耗时 {:.1}秒，请重启应用程序以完成更新",
                                check_result.latest_version,
                                elapsed_ms as f64 / 1000.0
                            ),
                        },
                    );
                    tracing::info!(target: "UpdateCmd", "更新安装成功");
                }
                Err(e) => {
                    emit_update_state(
                        &app,
                        UpdateState::Failed {
                            error: e.to_string(),
                            version: check_result.latest_version.clone(),
                            stage: "installation".to_string(),
                            message: format!("安装失败: {}", e),
                        },
                    );
                    return Err(e.to_string());
                }
            }
        }
        Err(e) => {
            remove_update_download_cancel(&download_id_for_task);
            // 发送失败状态
            emit_update_state(
                &app,
                UpdateState::Failed {
                    error: e.clone(),
                    version: check_result.latest_version.clone(),
                    stage: "download".to_string(),
                    message: format!("下载失败: {}", e),
                },
            );
            return Err(e);
        }
    }

    Ok(())
}

/// 带重试机制的下载函数
async fn download_with_retry(
    app: &AppHandle,
    url: &str,
    dest: &std::path::Path,
    total_size: u64,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    version: &str,
    max_retries: u32,
) -> Result<crate::update::download::UpdateDownloadResult, String> {
    let mut attempt = 0;

    loop {
        attempt += 1;

        // 取消检查
        if *cancel_rx.borrow() {
            return Err("下载已取消".to_string());
        }

        tracing::info!(target: "UpdateDownload", attempt = attempt, version = %version, "开始更新下载尝试");

        // 发送下载开始/重试状态
        if attempt == 1 {
            emit_update_state(
                app,
                UpdateState::DownloadStarted {
                    total_bytes: total_size,
                    version: version.to_string(),
                    message: format!(
                        "开始下载版本 {}，大小 {:.1}MB",
                        version,
                        total_size as f64 / 1_048_576.0
                    ),
                },
            );
        } else {
            emit_update_state(
                app,
                UpdateState::DownloadProgress {
                    progress: 0.0,
                    downloaded: 0,
                    total: total_size,
                    speed_mbps: 0.0,
                    eta_secs: None,
                    message: format!("重试第 {} 次下载...", attempt),
                },
            );
        }

        match crate::update::download::download_update(
            app,
            url,
            dest,
            total_size,
            cancel_rx.clone(),
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::warn!(
                    target: "UpdateDownload",
                    attempt = attempt,
                    error = %e,
                    version = %version,
                    "更新下载失败"
                );

                if attempt >= max_retries {
                    return Err(format!(
                        "更新下载失败，已重试{}次，版本 {}: {}",
                        max_retries, version, e
                    ));
                }

                // 指数退避：3s, 6s, 12s
                let delay = 3u64.pow(attempt);
                tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
            }
        }
    }
}

/// 验证下载文件完整性（同步版本）
fn verify_download_file(
    file_path: &str,
    expected_size: u64,
    expected_sha256: &Option<String>,
    app: &AppHandle,
    version: &str,
) -> Result<(), String> {
    let path = std::path::Path::new(file_path);

    // 1. 检查文件是否存在
    if !path.exists() {
        return Err(format!("更新包文件不存在: {}", file_path));
    }

    // 2. 检查文件大小
    let metadata = std::fs::metadata(path).map_err(|e| format!("无法读取更新包元数据: {}", e))?;

    if expected_size > 0 && metadata.len() != expected_size {
        return Err(format!(
            "更新包大小不匹配: 期望 {} 字节，实际 {} 字节，版本: {}",
            expected_size,
            metadata.len(),
            version
        ));
    }

    // 3. 计算SHA256
    let sha256 = crate::update::download::compute_sha256(path);

    // 4. 验证SHA256
    match expected_sha256 {
        Some(expected) => {
            if sha256 != Some(expected.clone()) {
                return Err(format!(
                    "更新包SHA256校验失败: 期望 {}, 实际 {}, 版本: {}",
                    expected,
                    sha256.unwrap_or_else(|| "计算失败".to_string()),
                    version
                ));
            }
        }
        None => {
            // 签名验证已在 Manifest 检查阶段完成；SHA256 作为额外的完整性校验（纵深防御）。
            // 若 Manifest 未提供 SHA256，拒绝安装，避免安装被篡改但签名合法的包。
            tracing::error!(
                target: "UpdateDownload",
                version = version,
                "更新包缺少 SHA256 完整性校验，拒绝安装"
            );
            return Err(format!(
                "更新包完整性校验缺失：缺少 SHA256，版本：{}",
                version
            ));
        }
    }

    // 5. 发送验证进度
    let progress = UpdateDownloadProgress {
        stage: "verifying".to_string(),
        progress: 1.0,
        downloaded: metadata.len(),
        total: expected_size,
        speed_mbps: 0.0,
        eta_secs: None,
        message: format!("更新包验证完成，版本: {}", version),
        version: Some(version.to_string()),
        step: Some("verification_complete".to_string()),
    };

    let _ = app.emit(EVT_UPDATE_DOWNLOAD_PROGRESS, progress);

    Ok(())
}

/// 取消当前正在进行的更新下载
#[tauri::command]
pub async fn cancel_update_download(
    app: AppHandle,
    download_id: Option<String>,
) -> Result<(), String> {
    if let Some(ref download_id) = download_id {
        // 取消指定下载任务
        if let Ok(guard) = UPDATE_DOWNLOAD_CANCELS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
        {
            if let Some(sender) = guard.get(download_id) {
                let _ = sender.send(true);
            }
        }
    } else {
        // 兼容旧代码：取消所有下载任务
        if let Ok(guard) = UPDATE_DOWNLOAD_CANCELS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
        {
            for sender in guard.values() {
                let _ = sender.send(true);
            }
        }
    }
    emit_update_state(
        &app,
        UpdateState::Cancelled {
            message: "更新已取消".to_string(),
        },
    );
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
            version: None,
            step: None,
        },
    );

    Ok(())
}

/// 检查更新（异步，不阻塞事件循环）
#[tauri::command]
pub async fn check_updates() -> Result<UpdateCheckResult, String> {
    tracing::info!(target: "UpdateCmd", "收到检查更新请求");
    check_for_updates().await.map_err(|e| {
        tracing::error!(target: "UpdateCmd", error = %e, "检查更新失败");
        format!("检查更新失败：{}", e)
    })
}

/// 清理旧版本
#[tauri::command]
pub fn cleanup_old_version(path: String) -> Result<(), String> {
    cleanup_old_installation(&path).map_err(|e| format!("清理失败：{}", e))
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
