//! 日志导出命令。
//!
//! 支持将运行日志导出为 txt/json/csv 格式。

use serde::{Deserialize, Serialize};
use tauri::State;
use crate::server::LogLine;
use super::AppState;
use std::fs;

/// 导出格式枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// 纯文本格式（每行：时间 [流] 文本）
    Text,
    /// JSON 格式（数组，每行一个 LogLine）
    Json,
    /// CSV 格式（表头：timestamp,stream,text）
    Csv,
}

/// 导出请求参数
#[derive(Debug, Deserialize)]
pub struct ExportLogsRequest {
    /// 导出格式
    pub format: ExportFormat,
    /// 导出路径（由前端通过文件对话框选择）
    pub path: String,
    /// 导出范围：all / visible / selected_group
    #[serde(default = "default_export_scope")]
    pub scope: String,
    /// 如果 scope 是 selected_group，指定分组 ID
    #[serde(default)]
    pub group_id: Option<String>,
}

fn default_export_scope() -> String {
    "all".to_string()
}

/// 导出日志到文件
#[tauri::command]
pub fn export_logs(
    state: State<'_, AppState>,
    req: ExportLogsRequest,
) -> Result<(), String> {
    let logs = state.server.logs_snapshot();
    
    // 根据 scope 过滤日志
    let filtered = match req.scope.as_str() {
        "visible" => {
            // 前端会传递当前可见的日志行 ID，这里简化为全部
            logs
        }
        "selected_group" => {
            if let Some(ref gid) = req.group_id {
                logs.into_iter()
                    .filter(|l| l.group.as_ref() == Some(gid))
                    .collect()
            } else {
                logs
            }
        }
        _ => logs, // "all"
    };
    
    // 格式化输出
    let content = match req.format {
        ExportFormat::Text => format_as_text(&filtered),
        ExportFormat::Json => format_as_json(&filtered),
        ExportFormat::Csv => format_as_csv(&filtered),
    };
    
    // 写入文件
    fs::write(&req.path, content)
        .map_err(|e| format!("写入文件失败：{}", e))
}

fn format_as_text(logs: &[LogLine]) -> String {
    logs.iter()
        .map(|l| format!("{} [{}] {}", l.timestamp, l.stream, l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_as_json(logs: &[LogLine]) -> String {
    serde_json::to_string_pretty(logs).unwrap_or_default()
}

fn format_as_csv(logs: &[LogLine]) -> String {
    let mut out = String::from("timestamp,stream,text\n");
    for l in logs {
        // CSV 转义：文本中的双引号加倍，字段用双引号包裹
        let text = l.text.replace('"', "\"\"");
        out.push_str(&format!("\"{}\",\"{}\",\"{}\"\n", 
            l.timestamp, l.stream, text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn format_text_basic() {
        let logs = vec![
            LogLine::plain("stdout", "hello"),
            LogLine::grouped("stderr", "error", "init"),
        ];
        let out = format_as_text(&logs);
        assert!(out.contains("[stdout] hello"));
        assert!(out.contains("[stderr] error"));
    }
    
    #[test]
    fn format_csv_escapes_quotes() {
        let logs = vec![LogLine::plain("stdout", "say \"hello\"")];
        let out = format_as_csv(&logs);
        assert!(out.contains("\"say \"\"hello\"\"\""));
    }
    
    #[test]
    fn format_json_valid() {
        let logs = vec![LogLine::plain("stdout", "test")];
        let out = format_as_json(&logs);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }
}
