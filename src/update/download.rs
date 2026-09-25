//! 更新下载模块。
//!
//! 下载更新压缩包，支持进度实时推送、取消、SHA256 校验。
//! 使用异步流式下载（`NetClient` + `bytes_stream`），避免大文件 OOM。

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result as AnyResult;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tracing::info;

use crate::events::{UpdateDownloadProgress, EVT_UPDATE_DOWNLOAD_PROGRESS};
use crate::net::NetClient;
use crate::util::progress::ProgressReporter;

/// 下载结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDownloadResult {
    /// 下载的压缩包完整路径
    pub download_path: String,
    /// 文件大小（字节）
    pub file_size: u64,
    /// SHA256 校验和（下载完成后计算）
    pub sha256: Option<String>,
    /// 下载耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 下载进度阶段常量
const STAGE_INIT: &str = "init";
const STAGE_DOWNLOADING: &str = "downloading";
const STAGE_COMPLETED: &str = "completed";
const STAGE_CANCELLED: &str = "cancelled";
const STAGE_FAILED: &str = "failed";

/// 发送下载进度事件。
fn emit_progress(app: &AppHandle, progress: UpdateDownloadProgress) {
    let _ = app.emit(EVT_UPDATE_DOWNLOAD_PROGRESS, progress);
}

/// 异步下载更新包。
///
/// 进度通过 `EVT_UPDATE_DOWNLOAD_PROGRESS` 事件推送给前端。
/// 使用 `NetClient` 异步流式下载，避免大文件内存溢出。
pub async fn download_update(
    app: &AppHandle,
    download_url: &str,
    dest_path: &Path,
    expected_size: u64,
) -> AnyResult<UpdateDownloadResult> {
    let start_time = std::time::Instant::now();

    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)?;
    }

    emit_progress(app, UpdateDownloadProgress {
        stage: STAGE_INIT.to_string(),
        progress: 0.0,
        downloaded: 0,
        total: expected_size,
        speed_mbps: 0.0,
        eta_secs: None,
        message: "准备下载更新包...".to_string(),
    });

    let client = NetClient::builder()
        .user_agent("LlamaUI-Update/1.0")
        .build()?;

    if crate::update::UPDATE_DOWNLOAD_CANCEL.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("下载已取消"));
    }

    let response = client.send_get(download_url, &[]).await?;
    let status = response.status();
    if !status.is_success() && status.as_u16() != 206 {
        let msg = format!("下载失败：HTTP {}", status.as_u16());
        emit_progress(app, UpdateDownloadProgress {
            stage: STAGE_FAILED.to_string(),
            progress: 0.0,
            downloaded: 0,
            total: expected_size,
            speed_mbps: 0.0,
            eta_secs: None,
            message: msg.clone(),
        });
        return Err(anyhow::anyhow!("{}", msg));
    }

    let total = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(expected_size);

    // 异步流式下载
    let mut downloaded: u64 = 0;
    let mut file = fs::File::create(dest_path)?;
    let mut stream = response.bytes_stream();
    let mut progress_reporter = ProgressReporter::new(total, 10, Duration::from_millis(500));

    while let Some(chunk_result) = stream.next().await {
        if crate::update::UPDATE_DOWNLOAD_CANCEL.load(Ordering::Relaxed) {
            let _ = fs::remove_file(dest_path);
            emit_progress(app, UpdateDownloadProgress {
                stage: STAGE_CANCELLED.to_string(),
                progress: 0.0,
                downloaded,
                total,
                speed_mbps: 0.0,
                eta_secs: None,
                message: "下载已取消".to_string(),
            });
            return Err(anyhow::anyhow!("下载已取消"));
        }

        let chunk = chunk_result?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;

        if let Some((progress, dl, speed, eta)) = progress_reporter.observe(downloaded) {
            emit_progress(app, UpdateDownloadProgress {
                stage: STAGE_DOWNLOADING.to_string(),
                progress,
                downloaded: dl,
                total,
                speed_mbps: speed / 1_048_576.0,
                eta_secs: eta,
                message: format!(
                    "下载中 {:.1}%（{:.1} MB / {:.1} MB，{:.1} MB/s）",
                    progress * 100.0,
                    dl as f64 / 1_048_576.0,
                    total as f64 / 1_048_576.0,
                    speed / 1_048_576.0
                ),
            });
        }
    }

    file.flush()?;
    drop(file);

    // P0-5: 下载完成后校验文件大小
    if total > 0 && downloaded != total {
        let _ = fs::remove_file(dest_path);
        return Err(anyhow::anyhow!(
            "文件大小不匹配：期望 {}，实际 {}",
            total,
            downloaded
        ));
    }

    let sha256 = compute_sha256(dest_path);
    let elapsed_ms = start_time.elapsed().as_millis() as u64;

    info!(
        target: "UpdateDownload",
        url = %download_url,
        size = downloaded,
        "更新包下载完成"
    );

    emit_progress(app, UpdateDownloadProgress {
        stage: STAGE_COMPLETED.to_string(),
        progress: 1.0,
        downloaded,
        total,
        speed_mbps: 0.0,
        eta_secs: None,
        message: "下载完成".to_string(),
    });

    Ok(UpdateDownloadResult {
        download_path: dest_path.to_string_lossy().to_string(),
        file_size: downloaded,
        sha256,
        elapsed_ms,
    })
}

/// 计算文件 SHA256 校验和。
fn compute_sha256(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let data = fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
        Some(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_result_serialization() {
        let result = UpdateDownloadResult {
            download_path: "/tmp/update.zip".to_string(),
            file_size: 1024,
            sha256: Some("abc123".to_string()),
            elapsed_ms: 5000,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: UpdateDownloadResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.download_path, "/tmp/update.zip");
        assert_eq!(back.file_size, 1024);
        assert_eq!(back.sha256, Some("abc123".to_string()));
    }

    #[test]
    fn progress_serialization() {
        let progress = UpdateDownloadProgress {
            stage: "downloading".to_string(),
            progress: 0.5,
            downloaded: 512,
            total: 1024,
            speed_mbps: 1.5,
            eta_secs: Some(10),
            message: "下载中...".to_string(),
        };
        let json = serde_json::to_string(&progress).unwrap();
        let back: UpdateDownloadProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stage, "downloading");
        assert_eq!(back.progress, 0.5);
    }

    #[test]
    fn compute_sha256_known_value() {
        let dir = std::env::temp_dir().join("llamaui_sha256_test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.txt");
        fs::write(&path, b"").unwrap();
        let hash = compute_sha256(&path);
        assert_eq!(
            hash,
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
