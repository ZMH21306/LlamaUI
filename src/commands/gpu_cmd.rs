//! GPU 检测与诊断命令。
//!
//! 薄适配层：只做「IPC 参数 → 领域 API → 错误转换」。
//! 所有检测/诊断逻辑在 [`crate::gpu::detection`]，缓存由领域 API 内部实现，
//! 本层不再维护第二套缓存（旧 `GpuCache` 每次调用新建实例、永不命中，属无效代码，已删除）。

use std::time::Instant;

use crate::gpu::detection::{GpuInfo, GpuIssue};

/// 检测系统中的全部 GPU。
#[tauri::command]
pub async fn detect_gpus() -> Result<Vec<GpuInfo>, String> {
    let start = Instant::now();
    let gpus = crate::gpu::detection::detect_all_gpus_async()
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(
        target: "GpuCmd",
        count = gpus.len(),
        elapsed_ms = start.elapsed().as_millis(),
        "GPU 检测完成"
    );
    Ok(gpus)
}

/// 诊断 GPU 相关问题。
#[tauri::command]
pub async fn diagnose_gpu() -> Result<Vec<GpuIssue>, String> {
    let start = Instant::now();
    let issues = crate::gpu::detection::diagnose_gpu_issues_async()
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(
        target: "GpuCmd",
        count = issues.len(),
        elapsed_ms = start.elapsed().as_millis(),
        "GPU 诊断完成"
    );
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detect_gpus_returns_without_panicking() {
        // 无 GPU 环境会返回空列表或错误；这里只验证调用路径不 panic。
        let _ = detect_gpus().await;
    }

    #[tokio::test]
    async fn diagnose_gpu_returns_without_panicking() {
        let _ = diagnose_gpu().await;
    }
}