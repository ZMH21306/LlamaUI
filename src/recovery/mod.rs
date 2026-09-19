//! 错误诊断与恢复建议模块。
//!
//! 分析当前系统状态和配置，提供可操作的修复建议。
//!
//! - [`diagnostic`]：`diagnose` / `DiagnosisResult` / `DiagnosisIssue` / `IssueType`

pub mod diagnostic;

pub use diagnostic::{diagnose, DiagnosisIssue, DiagnosisResult, IssueType};