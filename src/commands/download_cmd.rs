//! llama.cpp 自动下载命令。

use crate::llama_downloader::{
    detect_gpu_backend, download_and_install, DownloadProgress, DownloadResult, GpuBackend,
};
use std::path::PathBuf;
use tauri::Emitter;

/// 下载并安装 llama-server（通过 Tauri event 实时推送进度）
#[tauri::command]
pub async fn download_llama_server(
    app: tauri::AppHandle,
    install_dir: Option<String>,
    backend: Option<String>,
) -> Result<DownloadResult, String> {
    // 确定安装目录
    let dir = install_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".llamaui")
                .join("llama-cpp")
        });

    // 确定 GPU 后端
    let gpu_backend = backend
        .map(|b| GpuBackend::from_str(&b))
        .unwrap_or_else(detect_gpu_backend);

    tracing::info!(
        target: "DownloadCmd",
        dir = %dir.display(),
        backend = %gpu_backend.as_str(),
        "开始下载"
    );

    // 发送开始事件
    let _ = app.emit(
        "download-progress",
        DownloadProgress {
            stage: "init".into(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: format!("开始下载 (后端: {})", gpu_backend.as_str()),
        },
    );

    // 克隆 AppHandle 用于 spawn_blocking 中的回调
    let app_clone = app.clone();

    // 执行下载，实时推送进度
    let result = tokio::task::spawn_blocking(move || {
        download_and_install(gpu_backend, &dir, Some(&|progress| {
            tracing::debug!(
                target: "DownloadCmd",
                stage = %progress.stage,
                progress = progress.progress,
                downloaded = progress.downloaded,
                total = progress.total,
                message = %progress.message,
                "下载进度"
            );
            let _ = app_clone.emit("download-progress", &progress);
        }))
    })
    .await
    .map_err(|e| {
        let msg = format!("下载任务执行失败: {}", e);
        tracing::error!(target: "DownloadCmd", error = %e, "spawn_blocking 失败");
        msg
    })?
    .map_err(|e| {
        let msg = format!("{}", e);
        tracing::error!(target: "DownloadCmd", error = %e, "下载安装失败");
        msg
    })?;

    tracing::info!(
        target: "DownloadCmd",
        path = ?result.path,
        file_size = result.file_size,
        elapsed_ms = result.elapsed_ms,
        "下载完成"
    );

    // 发送完成事件
    let _ = app.emit(
        "download-progress",
        DownloadProgress {
            stage: "complete".into(),
            progress: 1.0,
            downloaded: result.file_size,
            total: result.file_size,
            message: "下载完成".into(),
        },
    );

    Ok(result)
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
