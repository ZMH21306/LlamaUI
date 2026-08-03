//! GPU 检测命令。

use crate::gpu_detect::{GpuInfo, GpuIssue};

/// 检测所有 GPU
#[tauri::command]
pub fn detect_gpus() -> Result<Vec<GpuInfo>, String> {
    Ok(crate::gpu_detect::detect_all_gpus())
}

/// 诊断 GPU 问题
#[tauri::command]
pub fn diagnose_gpu() -> Result<Vec<GpuIssue>, String> {
    Ok(crate::gpu_detect::diagnose_gpu_issues())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_gpus() {
        let gpus = detect_gpus().unwrap();
        assert!(!gpus.is_empty());
    }

    #[test]
    fn test_diagnose_gpu() {
        let _issues = diagnose_gpu().unwrap();
    }
}
