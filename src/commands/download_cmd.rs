//! llama.cpp 自动下载命令。

use crate::download::llama_downloader::{
    detect_gpu_backend, download_and_install, DownloadProgress, DownloadResult, GpuBackend,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::Emitter;
use tauri::State;

use crate::commands::AppState;
use crate::events::{DownloadLogEntry, DownloadState, EVT_DOWNLOAD_PROGRESS, EVT_DOWNLOAD_STATE};

/// 下载并安装 llama-server（通过 Tauri event 实时推送进度）
#[tauri::command]
pub async fn download_llama_server(
    app: tauri::AppHandle,
    install_dir: Option<String>,
    backend: Option<String>,
    state: State<'_, AppState>,
) -> Result<DownloadResult, String> {
    // 重置取消标志
    state.download_cancel.store(false, Ordering::Relaxed);

    // 确定安装目录
    let dir = install_dir.map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".llamaui")
            .join("llama-cpp")
    });

    // 确定 GPU 后端
    let gpu_backend = backend
        .map(|b| GpuBackend::parse_backend(&b))
        .unwrap_or_else(detect_gpu_backend);

    tracing::info!(
        target: "DownloadCmd",
        dir = %dir.display(),
        backend = %gpu_backend.as_str(),
        "开始下载"
    );

    // 发送开始状态 + 立即切换到 Running，确保前端 UI 同步
    let _ = app.emit(
        EVT_DOWNLOAD_STATE,
        DownloadState::Started {
            backend: gpu_backend.as_str().to_string(),
            log_message: Some(format!("开始下载 llama-server (后端: {})", gpu_backend.as_str())),
        },
    );
    // 立即切换到 Running 状态，确保前端 UI 知道下载已开始
    let _ = app.emit(EVT_DOWNLOAD_STATE, DownloadState::Running);
    // 初始进度事件（optional，可不发射，因为后面会持续发送进度）
    let _ = app.emit(
        EVT_DOWNLOAD_PROGRESS,
        DownloadProgress {
            stage: "init".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: format!("正在下载 (后端: {})", gpu_backend.as_str()),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        },
    );

    // 克隆 AppHandle 用于 spawn_blocking 中的回调
    let app_clone = app.clone();
    let cancel_flag = state.download_cancel.clone();

    // 执行下载，实时推送进度
    let result = tokio::task::spawn_blocking(move || {
        download_and_install(
            gpu_backend,
            &dir,
            Some(&|progress: DownloadProgress| {
                tracing::debug!(
                    target: "DownloadCmd",
                    stage = %progress.stage,
                    progress = progress.progress,
                    downloaded = progress.downloaded,
                    total = progress.total,
                    message = %progress.message,
                    "下载进度"
                );
                // 同时发射状态和进度事件，确保前端同步
                let _ = app_clone.emit(EVT_DOWNLOAD_STATE, DownloadState::Running);
                let _ = app_clone.emit(EVT_DOWNLOAD_PROGRESS, &progress);
            }),
            Some(&cancel_flag),
        )
    })
    .await
    .map_err(|e| {
        let msg = format!("下载任务执行失败: {}", e);
        tracing::error!(target: "DownloadCmd", error = %e, "spawn_blocking 失败");
        // 发送失败状态
        let _ = app.emit(
            EVT_DOWNLOAD_STATE,
            DownloadState::Failed {
                error: msg.clone(),
                log_entry: Some(DownloadLogEntry {
                    message: msg.clone(),
                    level: "error".to_string(),
                    stage: "error".to_string(),
                    auto_scroll: true,
                }),
            },
        );
        msg
    })?
    .map_err(|e| {
        let msg = format!("{}", e);
        tracing::error!(target: "DownloadCmd", error = %e, "下载安装失败");
        // 如果是取消导致的错误，发送 Cancelled 状态
        if msg.contains("取消") || msg.contains("cancelled") {
            let _ = app.emit(EVT_DOWNLOAD_STATE, DownloadState::Cancelled);
        } else {
            let _ = app.emit(
                EVT_DOWNLOAD_STATE,
                DownloadState::Failed {
                    error: msg.clone(),
                    log_entry: Some(DownloadLogEntry {
                        message: msg.clone(),
                        level: "error".to_string(),
                        stage: "error".to_string(),
                        auto_scroll: true,
                    }),
                },
            );
        }
        msg
    })?;

    // 发送完成状态
    let _ = app.emit(
        EVT_DOWNLOAD_STATE,
        DownloadState::Completed {
            path: result.path.clone(),
            file_size: result.file_size,
            sha256: result.sha256.clone(),
            elapsed_ms: result.elapsed_ms,
            log_message: Some(format!(
                "下载完成: {} ({:.1} MB, {:.2}s)",
                result.path,
                result.file_size as f64 / 1_048_576.0,
                result.elapsed_ms as f64 / 1000.0
            )),
        },
    );

    tracing::info!(
        target: "DownloadCmd",
        path = ?result.path,
        file_size = result.file_size,
        elapsed_ms = result.elapsed_ms,
        "下载完成"
    );

    Ok(result)
}

/// 取消当前正在进行的 llama-server 下载
#[tauri::command]
pub async fn cancel_download_llama_server(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // 设置取消标志
    state.download_cancel.store(true, Ordering::Relaxed);
    tracing::info!(target: "DownloadCmd", "收到取消下载请求");
    
    // 发送正在取消状态
    let _ = app.emit(EVT_DOWNLOAD_STATE, DownloadState::Cancelling);
    
    // 等待一小段时间让下载线程检查取消标志
    std::thread::sleep(std::time::Duration::from_millis(100));
    
    Ok(())
}

/// 检测 GPU 后端
#[tauri::command]
pub fn detect_gpu() -> Result<String, String> {
    let backend = detect_gpu_backend();
    Ok(backend.as_str().to_string())
}

/// 获取可用的 GPU 后端列表
#[tauri::command]
pub fn list_gpu_backends() -> Vec<String> {
    let mut backends = vec!["cpu".to_string()];

    // 根据平台添加可能的后端
    let os = std::env::consts::OS;
    match os {
        "windows" => {
            backends.push("cuda".to_string());
            backends.push("vulkan".to_string());
        }
        "linux" => {
            backends.push("cuda".to_string());
            backends.push("rocm".to_string());
            backends.push("vulkan".to_string());
        }
        "macos" => {
            backends.push("metal".to_string());
        }
        _ => {}
    }

    backends
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_gpu() {
        let backend = detect_gpu().unwrap();
        assert!(!backend.is_empty());
    }

    #[test]
    fn test_list_backends() {
        let backends = list_gpu_backends();
        assert!(backends.contains(&"cpu".to_string()));
    }
}
