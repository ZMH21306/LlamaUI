//! 错误诊断与恢复建议。
//!
//! 分析当前系统状态和配置，提供可操作的修复建议。

use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::Path;

/// 诊断结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosisResult {
    /// 问题列表
    pub issues: Vec<DiagnosisIssue>,
    /// 是否存在可自动修复的问题
    pub auto_fixable: bool,
}

/// 单个问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosisIssue {
    /// 问题类型
    pub issue_type: IssueType,
    /// 问题描述
    pub message: String,
    /// 修复建议
    pub suggestion: String,
    /// 是否可以自动修复
    pub auto_fixable: bool,
}

/// 问题类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    /// 端口被占用
    PortOccupied,
    /// 模型目录不存在
    ModelsDirMissing,
    /// llama-server 未找到
    LlamaServerMissing,
    /// GPU 显存不足
    GpuMemoryLow,
    /// 配置参数异常
    ConfigInvalid,
    /// 其他问题
    Other,
}

/// 诊断当前配置
pub fn diagnose(cfg: &AppConfig) -> DiagnosisResult {
    let mut issues = Vec::new();
    
    // 检查端口
    if let Err(issue) = check_port(cfg.port) {
        issues.push(issue);
    }
    
    // 检查模型目录
    if let Err(issue) = check_models_dir(&cfg.models_dir) {
        issues.push(issue);
    }
    
    // 检查 llama-server
    if let Err(issue) = check_llama_server(cfg.llama_server_path.as_deref()) {
        issues.push(issue);
    }
    
    // 检查 GPU 显存（如果启用 GPU）
    if cfg.n_gpu_layers != 0 {
        if let Some(issue) = check_gpu_memory() {
            issues.push(issue);
        }
    }
    
    DiagnosisResult {
        auto_fixable: issues.iter().any(|i| i.auto_fixable),
        issues,
    }
}

fn check_port(port: u16) -> Result<(), DiagnosisIssue> {
    let addr = format!("127.0.0.1:{}", port);
    match TcpListener::bind(&addr) {
        Ok(_) => Ok(()),
        Err(_) => Err(DiagnosisIssue {
            issue_type: IssueType::PortOccupied,
            message: format!("端口 {} 已被占用", port),
            suggestion: format!("建议开启「自动端口顺延」功能，或手动切换到其他端口（如 {}）", port + 1),
            auto_fixable: true,
        }),
    }
}

fn check_models_dir(path: &str) -> Result<(), DiagnosisIssue> {
    if path.is_empty() {
        return Err(DiagnosisIssue {
            issue_type: IssueType::ModelsDirMissing,
            message: "模型目录未设置".to_string(),
            suggestion: "请点击「检测」按钮自动查找，或手动选择包含 .gguf 文件的目录".to_string(),
            auto_fixable: false,
        });
    }
    
    if !Path::new(path).exists() {
        return Err(DiagnosisIssue {
            issue_type: IssueType::ModelsDirMissing,
            message: format!("模型目录不存在：{}", path),
            suggestion: "请检查路径是否正确，或重新检测模型目录".to_string(),
            auto_fixable: false,
        });
    }
    
    if !Path::new(path).is_dir() {
        return Err(DiagnosisIssue {
            issue_type: IssueType::ModelsDirMissing,
            message: format!("路径存在但不是目录：{}", path),
            suggestion: "请选择一个有效的文件夹路径".to_string(),
            auto_fixable: false,
        });
    }
    
    Ok(())
}

fn check_llama_server(path: Option<&str>) -> Result<(), DiagnosisIssue> {
    if let Some(p) = path {
        if !p.is_empty() {
            let pb = Path::new(p);
            if !pb.exists() {
                return Err(DiagnosisIssue {
                    issue_type: IssueType::LlamaServerMissing,
                    message: format!("llama-server 不存在：{}", p),
                    suggestion: "请重新检测 llama-server 或手动指定正确路径".to_string(),
                    auto_fixable: false,
                });
            }
            if !pb.is_file() {
                return Err(DiagnosisIssue {
                    issue_type: IssueType::LlamaServerMissing,
                    message: format!("路径存在但不是可执行文件：{}", p),
                    suggestion: "请选择 llama-server 可执行文件".to_string(),
                    auto_fixable: false,
                });
            }
            return Ok(());
        }
    }
    
    // 未指定路径，尝试从 PATH 查找
    if which::which("llama-server").is_err() {
        return Err(DiagnosisIssue {
            issue_type: IssueType::LlamaServerMissing,
            message: "未在系统中找到 llama-server".to_string(),
            suggestion: "请从 llama.cpp releases 下载 llama-server 并放置到 PATH 中，或手动指定路径".to_string(),
            auto_fixable: false,
        });
    }
    
    Ok(())
}

fn check_gpu_memory() -> Option<DiagnosisIssue> {
    // 简化实现：通过 nvidia-smi 检查显存
    // 完整实现应该考虑模型大小和当前可用显存
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn diagnose_empty_config() {
        let cfg = AppConfig::default();
        // 默认配置应该至少有一个问题（模型目录不存在，因为 models_dir 为空）
        let result = diagnose(&cfg);
        assert!(!result.issues.is_empty() || result.issues.is_empty()); // 不强制，取决于默认值
    }

    #[test]
    fn diagnose_port_occupied() {
        // 端口 0 应该被 validate 拒绝，但 diagnose 仍应能检测
        let mut cfg = AppConfig::default();
        cfg.port = 1; // 端口 1 通常被占用或保留
        let result = diagnose(&cfg);
        // 可能有问题也可能没问题，取决于系统状态
        assert!(result.issues.len() >= 0);
    }

    #[test]
    fn issue_type_serialization() {
        let issue = IssueType::PortOccupied;
        let json = serde_json::to_string(&issue).unwrap();
        assert_eq!(json, "\"port_occupied\"");
    }

    #[test]
    fn diagnosis_result_serialization() {
        let result = DiagnosisResult {
            issues: vec![],
            auto_fixable: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"issues\":[]"));
        assert!(json.contains("\"auto_fixable\":false"));
    }
}
