//! GPU 错误转换器。
//!
//! 将 GPU 检测和诊断过程中的技术错误转换为用户友好的提示信息。
//!
//! # 设计原则
//!
//! - 所有用户可见的错误信息使用中文
//! - 提供明确的操作建议
//! - 区分可自动修复和需要用户操作的问题

use serde::{Deserialize, Serialize};

/// 用户友好的 GPU 错误类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuError {
    /// 错误代码
    pub code: String,
    /// 用户友好的标题
    pub title: String,
    /// 详细描述
    pub description: String,
    /// 建议的操作
    pub action: String,
    /// 是否可自动修复
    pub auto_fixable: bool,
}

impl GpuError {
    /// 从错误类型创建用户友好的错误
    pub fn from_issue_type(issue_type: &str, message: &str, auto_fixable: bool) -> Self {
        match issue_type {
            "no_gpu" => Self {
                code: "GPU001".to_string(),
                title: "未检测到独立显卡".to_string(),
                description: message.to_string(),
                action: "如需 GPU 加速，请安装 NVIDIA、AMD 或 Apple Silicon 独立显卡".to_string(),
                auto_fixable: false,
            },
            "old_cuda" => Self {
                code: "GPU002".to_string(),
                title: "CUDA 版本过旧".to_string(),
                description: message.to_string(),
                action: "请更新 NVIDIA 驱动程序以获取最新 CUDA 支持".to_string(),
                auto_fixable: false,
            },
            "low_vram" => Self {
                code: "GPU003".to_string(),
                title: "显存容量较小".to_string(),
                description: message.to_string(),
                action: "建议使用较小的模型（如 7B 参数模型）或增加显存".to_string(),
                auto_fixable: false,
            },
            "no_rocm" => Self {
                code: "GPU004".to_string(),
                title: "未检测到 ROCm".to_string(),
                description: message.to_string(),
                action: "请安装 ROCm 驱动（Linux）或使用 Vulkan 后端".to_string(),
                auto_fixable: false,
            },
            "no_cuda_runtime" => Self {
                code: "GPU005".to_string(),
                title: "CUDA Runtime 不可用".to_string(),
                description: message.to_string(),
                action: "请安装 NVIDIA CUDA Toolkit".to_string(),
                auto_fixable: true,
            },
            _ => Self {
                code: "GPU999".to_string(),
                title: "GPU 检测异常".to_string(),
                description: message.to_string(),
                action: "请检查 GPU 驱动程序是否正确安装".to_string(),
                auto_fixable: false,
            },
        }
    }

    /// 检测失败的友好错误
    pub fn detection_failed(details: &str) -> Self {
        Self {
            code: "GPU100".to_string(),
            title: "GPU 检测失败".to_string(),
            description: format!("检测过程中出现错误：{}", details),
            action: "请确保 GPU 驱动程序已正确安装，或联系技术支持".to_string(),
            auto_fixable: false,
        }
    }

    /// 超时的友好错误
    pub fn timeout() -> Self {
        Self {
            code: "GPU101".to_string(),
            title: "GPU 检测超时".to_string(),
            description: "GPU 检测耗时过长，可能存在系统问题".to_string(),
            action: "请检查 GPU 是否正常工作，或尝试重启应用程序".to_string(),
            auto_fixable: false,
        }
    }
}

/// 将 GPU 问题转换为用户友好的错误
pub fn issue_to_error(issue_type: &str, message: &str, auto_fixable: bool) -> GpuError {
    GpuError::from_issue_type(issue_type, message, auto_fixable)
}

/// 将多个 GPU 问题转换为用户友好的错误列表
pub fn issues_to_errors(issues: &[(String, String, bool)]) -> Vec<GpuError> {
    issues
        .iter()
        .map(|(issue_type, message, auto_fixable)| {
            GpuError::from_issue_type(issue_type, message, *auto_fixable)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_gpu_error() {
        let error = GpuError::from_issue_type(
            "no_gpu",
            "未检测到独立 GPU，将使用 CPU 推理",
            false,
        );
        assert_eq!(error.code, "GPU001");
        assert_eq!(error.title, "未检测到独立显卡");
        assert!(!error.auto_fixable);
    }

    #[test]
    fn test_timeout_error() {
        let error = GpuError::timeout();
        assert_eq!(error.code, "GPU101");
        assert_eq!(error.title, "GPU 检测超时");
    }

    #[test]
    fn test_detection_failed_error() {
        let error = GpuError::detection_failed("nvidia-smi 执行失败");
        assert_eq!(error.code, "GPU100");
        assert!(error.description.contains("nvidia-smi"));
    }
}