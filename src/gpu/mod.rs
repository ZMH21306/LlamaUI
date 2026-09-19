//! GPU 检测与诊断模块。
//!
//! 检测系统 GPU 型号、驱动版本、CUDA/ROCm/Vulkan 版本，
//! 并提供自动修复建议。
//!
//! # 兼容性
//!
//! 原 `gpu_detect.rs` 是一个薄门面；`gpu_error_transformer.rs` 用于错误转换。
//! 在重构中：
//! - `gpu_detection` → `gpu::detection`（主要实现）
//! - `gpu_error_transformer` 已移除（与 detection 合并，未被外部引用）
//! - `gpu_detect`（薄门面）已移除，调用方直接使用异步 API

pub mod detection;