//! GPU 检测模块（兼容旧代码的轻量门面）。
//!
//! 新代码应优先使用 [`crate::gpu_detection`]。本模块保留以下项目以兼容旧代码：
//! - [`detect_all_gpus`]：同步包装，调用异步实现
//! - [`auto_fix_gpu_issue`]：自动修复
//! - 类型别名：旧代码可能引用了 `GpuInfo` / `GpuIssue`
//!
//! 异步实现位于 [`crate::gpu_detection`]，包含：
//! - [`crate::gpu_detection::detect_all_gpus_async`]：异步检测
//! - [`crate::gpu_detection::diagnose_gpu_issues_async`]：异步诊断
//! - [`crate::gpu_detection::GpuStateEvent`]：GPU 状态变更事件

use tokio::runtime::Runtime;

pub use crate::gpu_detection::{auto_fix_gpu_issue, diagnose_gpu_issues, GpuInfo, GpuIssue};

/// 检测所有 GPU（同步包装）
pub fn detect_all_gpus() -> Vec<GpuInfo> {
    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(target: "GpuDetect", "创建 tokio 运行时失败: {}", e);
            return Vec::new();
        }
    };
    runtime.block_on(crate::gpu_detection::detect_all_gpus_async()).unwrap_or_default()
}