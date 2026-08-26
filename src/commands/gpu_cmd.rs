//! GPU 检测命令。

use crate::gpu_detection::{self, GpuInfo, GpuIssue};

/// 检测所有 GPU
///
/// 异步检测系统中的所有 GPU（NVIDIA/AMD/Apple Silicon）。
/// 使用 tokio::join! 并发执行检测，超时保护避免长时间阻塞。
#[tauri::command]
pub async fn detect_gpus() -> Result<Vec<GpuInfo>, String> {
    // 性能监控：记录检测耗时
    let start = std::time::Instant::now();
    let result = gpu_detection::detect_all_gpus_async().await;
    let elapsed = start.elapsed();

    tracing::info!(
        target: "GpuCmd",
        count = result.len(),
        elapsed_ms = elapsed.as_millis() as u64,
        "GPU 检测完成"
    );

    Ok(result)
}

/// 诊断 GPU 问题
///
/// 异步诊断系统中的 GPU 问题（CUDA Runtime 缺失、显存不足等）。
/// 使用并发执行各项诊断。
#[tauri::command]
pub async fn diagnose_gpu() -> Result<Vec<GpuIssue>, String> {
    let issues = gpu_detection::diagnose_gpu_issues_async().await;

    tracing::info!(
        target: "GpuCmd",
        count = issues.len(),
        "GPU 诊断完成"
    );

    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_detect_gpus() {
        let gpus = detect_gpus().await.unwrap();
        assert!(!gpus.is_empty());
    }

    #[tokio::test]
    async fn test_diagnose_gpu() {
        let _issues = diagnose_gpu().await.unwrap();
    }
}