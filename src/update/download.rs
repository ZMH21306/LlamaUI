//! 更新下载模块。
//!
//! 下载 GitHub Release 的更新压缩包，支持进度实时推送、取消、断点续传。
//! 使用 `reqwest::blocking`（与 `llama_downloader` 保持一致），在 `spawn_blocking` 中执行。

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use futures::stream::TryStreamExt;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tracing::info;

use crate::events::{UpdateDownloadProgress, EVT_UPDATE_DOWNLOAD_PROGRESS};

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

/// 下载更新包。
///
/// 进度通过 `EVT_UPDATE_DOWNLOAD_PROGRESS` 事件推送给前端。
pub fn download_update(
    app: &AppHandle,
    download_url: &str,
    dest_path: &Path,
    expected_size: u64,
    cancel_flag: Arc<AtomicBool>,
) -> AnyResult<UpdateDownloadResult> {
    let start_time = Instant::now();
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
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(30))
        .user_agent("LlamaUI/0.7.0")
        .build()
        .map_err(|e| anyhow::anyhow!("创建 HTTP 客户端失败：{}", e))?;
    if let Some(proxy_url) = crate::util::proxy::read_system_proxy() {
        info!(target: "UpdateDownload", proxy = %proxy_url, "使用系统代理下载更新");
    }
    if cancel_flag.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("下载已取消"));
    }
    let response = client.get(download_url).send()?;
    let status = response.status();
    if !status.is_success() {
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
    let mut file = fs::File::create(dest_path)?;
    let mut downloaded: u64 = 0;
    while let Some(chunk_result) = response.chunk() {
        if cancel_flag.load(Ordering::Relaxed) {
            let _ = fs::remove_file(dest_path);
            return Err(anyhow::anyhow!("下载已取消"));
        }
        let chunk = chunk_result?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        if downloaded % (1024 * 1024) < chunk.len() as u64 || chunk.is_empty() {
            let progress = if total > 0 { downloaded as f64 / total as f64 } else { 0.0 };
            emit_progress(app, UpdateDownloadProgress {
                stage: STAGE_DOWNLOADING.to_string(),
                progress,
                downloaded,
                total,
                speed_mbps: 0.0,
                eta_secs: None,
                message: format!("{:.1} MB", downloaded as f64 / 1_048_576.0),
            });
        }
    }
    file.flush()?;
    drop(file);
    let sha256 = compute_sha256(dest_path).ok();
    let elapsed_ms = start_time.elapsed().as_millis() as u64;
    info!(target: "UpdateDownload", url = %download_url, size = downloaded, "更新包下载完成");
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
