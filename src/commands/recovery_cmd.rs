//! 错误诊断与恢复建议命令。

use tauri::State;
use crate::recovery::{diagnose, DiagnosisResult, IssueType};
use super::AppState;

/// 诊断当前配置
#[tauri::command]
pub fn get_diagnosis(state: State<'_, AppState>) -> DiagnosisResult {
    let cfg = state.config.get();
    diagnose(&cfg)
}

/// 自动修复可修复的问题
#[tauri::command]
pub fn auto_fix_issues(
    state: State<'_, AppState>,
    issue_types: Vec<IssueType>,
) -> Result<(), String> {
    let mut cfg = state.config.get();
    let mut fixed = false;
    
    for issue_type in issue_types {
        if let IssueType::PortOccupied = issue_type {
            // 自动顺延端口
            cfg.port = cfg.port.wrapping_add(1);
            if cfg.port == 0 {
                cfg.port = 10897; // 回退到默认端口
            }
            fixed = true;
        }
        // 其他问题需要用户手动修复
    }
    
    if fixed {
        state.config.set(cfg).map_err(|e| e.to_string())?;
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_type_roundtrip() {
        let types = [
            IssueType::PortOccupied,
            IssueType::ModelsDirMissing,
            IssueType::LlamaServerMissing,
            IssueType::GpuMemoryLow,
            IssueType::ConfigInvalid,
            IssueType::Other,
        ];
        for t in types {
            let json = serde_json::to_string(&t).unwrap();
            let back: IssueType = serde_json::from_str(&json).unwrap();
            assert_eq!(t, back);
        }
    }
}
