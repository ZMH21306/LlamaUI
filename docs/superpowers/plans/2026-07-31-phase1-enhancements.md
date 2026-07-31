# Phase 1 功能增强实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 LlamaUI 添加日志导出、配置备份、错误自动恢复和性能监控改进四个功能

**Architecture：** 后端新增 commands 模块处理新功能，前端在现有 UI 框架内扩展，通过 IPC 命令与后端通信

**Tech Stack：** Rust + Tauri 2.x（后端），HTML/CSS/JS（前端），serde_json（配置序列化）

---

## 文件结构

### 新增文件
- `src/commands/export_cmd.rs` - 日志导出 IPC 命令
- `src/commands/backup_cmd.rs` - 配置备份与恢复 IPC 命令
- `src/commands/recovery_cmd.rs` - 错误诊断与恢复建议 IPC 命令
- `src/backup.rs` - 配置备份管理模块
- `src/recovery.rs` - 错误诊断与恢复建议模块
- `src/metrics_enhanced.rs` - 增强版性能指标（滑动平均）

### 修改文件
- `src/commands/mod.rs` - 注册新命令模块
- `src/lib.rs` - 注册新的 IPC 命令
- `src/events.rs` - 新增事件常量
- `src/server/metrics.rs` - 改进统计方法
- `dist/main.js` - 前端 UI 扩展
- `dist/index.html` - 新增 UI 元素
- `dist/styles.css` - 新增样式

---

## Task 1: 日志导出功能

**Files:**
- Create: `src/commands/export_cmd.rs`
- Modify: `src/commands/mod.rs`, `src/lib.rs`, `dist/main.js`, `dist/index.html`

### Step 1: 创建日志导出 IPC 命令

```rust
// src/commands/export_cmd.rs
//! 日志导出命令。
//!
//! 支持将运行日志导出为 txt/json/csv 格式。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use crate::server::{LogLine, ServerProcess};
use super::AppState;
use std::fs;
use std::path::PathBuf;

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
}
```

- [ ] **Step 2: 在 commands/mod.rs 中注册模块**

在 `src/commands/mod.rs` 中添加：

```rust
pub mod export_cmd;
```

- [ ] **Step 3: 在 lib.rs 中注册 IPC 命令**

在 `src/lib.rs` 的 `.invoke_handler()` 中添加：

```rust
tauri::invoke_handler! {
    // ... 现有命令 ...
    commands::export_cmd::export_logs,
}
```

- [ ] **Step 4: 在前端添加导出按钮**

在 `dist/index.html` 的日志面板 header 中添加：

```html
<button class="btn-secondary btn-small" id="exportLogs" type="button" title="导出日志">导出</button>
```

- [ ] **Step 5: 在前端实现导出逻辑**

在 `dist/main.js` 中添加：

```javascript
// 导出日志
async function handleExportLogs() {
  const format = prompt('导出格式（txt/json/csv）：', 'txt');
  if (!format || !['txt', 'json', 'csv'].includes(format)) return;
  
  const path = await window.__TAURI__.dialog.save({
    defaultPath: `llamaui-logs-${Date.now()}.${format}`,
    filters: [{
      name: `日志文件 (*.\\${format})`,
      extensions: [format]
    }]
  });
  
  if (!path) return;
  
  try {
    await invoke('export_logs', {
      req: {
        format: format,
        path: path,
        scope: 'all'
      }
    });
    showToast('日志导出成功', 'success');
  } catch (e) {
    showToast('导出失败：' + e, 'error');
  }
}

// 绑定事件
$('exportLogs')?.addEventListener('click', handleExportLogs);
```

- [ ] **Step 6: 运行测试**

```bash
cargo test --lib export_cmd
cargo build --release
```

---

## Task 2: 配置备份功能

**Files:**
- Create: `src/commands/backup_cmd.rs`, `src/backup.rs`
- Modify: `src/commands/mod.rs`, `src/lib.rs`, `dist/main.js`, `dist/index.html`

### Step 1: 创建备份管理模块

```rust
// src/backup.rs
//! 配置备份管理。
//!
//! 自动保留最近 N 个备份，支持手动备份和恢复。

use crate::config::{AppConfig, config_path};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BACKUPS: usize = 5;

/// 备份元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMeta {
    /// 备份文件名
    pub filename: String,
    /// 备份时间戳
    pub timestamp: u64,
    /// 备份时的配置版本
    pub config_version: u32,
}

/// 获取备份目录
fn backup_dir() -> PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("LlamaUI").join("backups")
}

/// 创建新备份，返回备份文件名
pub fn create_backup(cfg: &AppConfig) -> anyhow::Result<String> {
    let dir = backup_dir();
    fs::create_dir_all(&dir)?;
    
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let filename = format!("config-{}.json", now);
    let path = dir.join(&filename);
    
    let data = serde_json::to_string_pretty(cfg)?;
    fs::write(&path, data)?;
    
    // 清理旧备份，保留最近 MAX_BACKUPS 个
    cleanup_old_backups()?;
    
    Ok(filename)
}

/// 列出所有备份
pub fn list_backups() -> Vec<BackupMeta> {
    let dir = backup_dir();
    if !dir.exists() {
        return Vec::new();
    }
    
    let mut backups = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("config-") && name_str.ends_with(".json") {
                // 从文件名提取时间戳
                let ts_str = &name_str[7..13]; // "config-".len() .. ".json".len()
                if let Ok(ts) = ts_str.parse::<u64>() {
                    backups.push(BackupMeta {
                        filename: name_str.to_string(),
                        timestamp: ts,
                        config_version: 0, // TODO: 从文件内容读取
                    });
                }
            }
        }
    }
    
    backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    backups
}

/// 恢复指定备份
pub fn restore_backup(filename: &str) -> anyhow::Result<AppConfig> {
    let path = backup_dir().join(filename);
    let data = fs::read_to_string(&path)?;
    let cfg: AppConfig = serde_json::from_str(&data)?;
    Ok(cfg)
}

/// 删除指定备份
pub fn delete_backup(filename: &str) -> anyhow::Result<()> {
    let path = backup_dir().join(filename);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// 清理旧备份，保留最近 MAX_BACKUPS 个
fn cleanup_old_backups() -> anyhow::Result<()> {
    let backups = list_backups();
    if backups.len() > MAX_BACKUPS {
        for backup in backups.iter().skip(MAX_BACKUPS) {
            let _ = delete_backup(&backup.filename);
        }
    }
    Ok(())
}
```

- [ ] **Step 2: 创建配置备份 IPC 命令**

```rust
// src/commands/backup_cmd.rs
//! 配置备份与恢复命令。

use tauri::State;
use crate::config::{AppConfig, ConfigStore};
use crate::backup;
use super::AppState;
use serde::{Deserialize, Serialize};

/// 备份响应
#[derive(Debug, Serialize)]
pub struct BackupResponse {
    pub filename: String,
    pub backups: Vec<backup::BackupMeta>,
}

/// 创建配置备份
#[tauri::command]
pub fn create_config_backup(
    state: State<'_, AppState>,
) -> Result<BackupResponse, String> {
    let cfg = state.config.get();
    let filename = backup::create_backup(&cfg)
        .map_err(|e| e.to_string())?;
    let backups = backup::list_backups();
    Ok(BackupResponse { filename, backups })
}

/// 列出所有备份
#[tauri::command]
pub fn list_config_backups() -> Result<Vec<backup::BackupMeta>, String> {
    backup::list_backups();
    Ok(backup::list_backups())
}

/// 恢复配置备份
#[tauri::command]
pub fn restore_config_backup(
    state: State<'_, AppState>,
    filename: String,
) -> Result<(), String> {
    let cfg = backup::restore_backup(&filename)
        .map_err(|e| e.to_string())?;
    state.config.set(cfg)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 删除配置备份
#[tauri::command]
pub fn delete_config_backup(
    filename: String,
) -> Result<(), String> {
    backup::delete_backup(&filename)
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 3: 注册模块和命令**

在 `src/commands/mod.rs` 添加 `pub mod backup_cmd;`
在 `src/lib.rs` 添加命令到 invoke_handler

- [ ] **Step 4: 在前端添加备份面板**

在 `dist/index.html` 设置面板中添加：

```html
<section class="card" id="backupSection">
  <header class="card-header">
    <h2>配置备份</h2>
  </header>
  <div class="card-body">
    <div class="backup-actions">
      <button class="btn-primary" id="createBackup">立即备份</button>
      <button class="btn-secondary" id="autoBackupToggle">自动备份：开</button>
    </div>
    <div class="backup-list" id="backupList">
      <!-- 动态生成 -->
    </div>
  </div>
</section>
```

- [ ] **Step 5: 实现前端备份逻辑**

在 `dist/main.js` 中添加备份相关的事件处理和 UI 更新。

- [ ] **Step 6: 测试**

```bash
cargo test --lib backup
cargo build --release
```

---

## Task 3: 错误自动恢复（基础版）

**Files:**
- Create: `src/commands/recovery_cmd.rs`, `src/recovery.rs`
- Modify: `src/commands/mod.rs`, `src/lib.rs`, `dist/main.js`, `dist/index.html`

### Step 1: 创建错误诊断模块

```rust
// src/recovery.rs
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
    /// 是否可以自动修复
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
            suggestion: format!("尝试使用自动端口顺延，或手动切换到其他端口（如 {}）", port + 1),
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
    
    Ok(())
}

fn check_llama_server(path: Option<&str>) -> Result<(), DiagnosisIssue> {
    if let Some(p) = path {
        if !p.is_empty() && !Path::new(p).exists() {
            return Err(DiagnosisIssue {
                issue_type: IssueType::LlamaServerMissing,
                message: format!("llama-server 不存在：{}", p),
                suggestion: "请重新检测 llama-server 或手动指定正确路径".to_string(),
                auto_fixable: false,
            });
        }
    }
    
    // 检查 PATH 中是否有 llama-server
    // 简化实现：假设如果没指定路径且没检测到，就有问题
    if path.is_none() || path.map_or(true, |p| p.is_empty()) {
        // 尝试从 PATH 查找
        if which::which("llama-server").is_err() {
            return Err(DiagnosisIssue {
                issue_type: IssueType::LlamaServerMissing,
                message: "未在系统中找到 llama-server".to_string(),
                suggestion: "请从 llama.cpp  releases 下载 llama-server 并放置到 PATH 中，或手动指定路径".to_string(),
                auto_fixable: false,
            });
        }
    }
    
    Ok(())
}

fn check_gpu_memory() -> Option<DiagnosisIssue> {
    // 简化实现：通过 nvidia-smi 检查显存
    // 实际应该更复杂，考虑模型大小和当前可用显存
    None
}
```

- [ ] **Step 2: 创建恢复建议 IPC 命令**

```rust
// src/commands/recovery_cmd.rs
use tauri::State;
use crate::recovery::{diagnose, IssueType};
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
    
    for issue_type in issue_types {
        match issue_type {
            IssueType::PortOccupied => {
                // 自动顺延端口
                cfg.port += 1;
            }
            // 其他问题需要用户手动修复
            _ => {}
        }
    }
    
    state.config.set(cfg).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 3: 注册模块和命令**

- [ ] **Step 4: 在前端添加诊断面板**

- [ ] **Step 5: 实现前端诊断逻辑**

- [ ] **Step 6: 测试**

---

## Task 4: 性能监控面板改进

**Files:**
- Create: `src/metrics_enhanced.rs`
- Modify: `src/server/metrics.rs`, `dist/main.js`, `dist/index.html`

### Step 1: 创建增强指标模块（滑动平均）

```rust
// src/metrics_enhanced.rs
//! 增强版性能指标：滑动平均 + 趋势指示 + 历史峰值

use crate::server::metrics::Metrics;
use std::collections::VecDeque;

const WINDOW_SIZE: usize = 5;

/// 滑动平均窗口
pub struct MetricsSmoother {
    window: VecDeque<Metrics>,
    peak_cpu: f32,
    peak_vram_pct: f32,
    peak_gpu_pct: f32,
}

impl MetricsSmoother {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW_SIZE),
            peak_cpu: 0.0,
            peak_vram_pct: 0.0,
            peak_gpu_pct: 0.0,
        }
    }
    
    /// 添加新采样，返回平滑后的指标和趋势
    pub fn push(&mut self, metrics: Metrics) -> SmoothedMetrics {
        self.window.push_back(metrics.clone());
        if self.window.len() > WINDOW_SIZE {
            self.window.pop_front();
        }
        
        // 更新峰值
        if metrics.cpu_percent > self.peak_cpu {
            self.peak_cpu = metrics.cpu_percent;
        }
        let vram_pct = if metrics.gpu_mem_total_mb > 0.0 {
            (metrics.gpu_mem_used_mb / metrics.gpu_mem_total_mb) * 100.0
        } else {
            0.0
        };
        if vram_pct > self.peak_vram_pct {
            self.peak_vram_pct = vram_pct;
        }
        if metrics.gpu_util_pct > self.peak_gpu_pct {
            self.peak_gpu_pct = metrics.gpu_util_pct;
        }
        
        // 计算滑动平均
        let avg = self.compute_average();
        
        // 计算趋势
        let trend = self.compute_trend();
        
        SmoothedMetrics {
            metrics: avg,
            trend,
            peak_cpu: self.peak_cpu,
            peak_vram_pct: self.peak_vram_pct,
            peak_gpu_pct: self.peak_gpu_pct,
        }
    }
    
    fn compute_average(&self) -> Metrics {
        if self.window.is_empty() {
            return Metrics::default();
        }
        
        let sum_cpu: f32 = self.window.iter().map(|m| m.cpu_percent).sum();
        let sum_virt: u64 = self.window.iter().map(|m| m.virtual_size_bytes).sum();
        let sum_gpu_used: f32 = self.window.iter().map(|m| m.gpu_mem_used_mb).sum();
        let sum_gpu_total: f32 = self.window.iter().map(|m| m.gpu_mem_total_mb).sum();
        let sum_gpu_util: f32 = self.window.iter().map(|m| m.gpu_util_pct).sum();
        let sum_app_mem: u64 = self.window.iter().map(|m| m.app_memory_bytes).sum();
        
        let n = self.window.len() as f32;
        Metrics {
            cpu_percent: sum_cpu / n,
            virtual_size_bytes: (sum_virt as f32 / n) as u64,
            gpu_mem_used_mb: sum_gpu_used / n,
            gpu_mem_total_mb: sum_gpu_total / n,
            gpu_util_pct: sum_gpu_util / n,
            app_memory_bytes: (sum_app_mem as f32 / n) as u64,
            ..self.window.back().cloned().unwrap_or_default()
        }
    }
    
    fn compute_trend(&self) -> Trend {
        if self.window.len() < 2 {
            return Trend::Stable;
        }
        
        let recent = self.window.back().unwrap();
        let older = self.window.front().unwrap();
        
        let cpu_delta = recent.cpu_percent - older.cpu_percent;
        if cpu_delta.abs() < 2.0 {
            Trend::Stable
        } else if cpu_delta > 0.0 {
            Trend::Up
        } else {
            Trend::Down
        }
    }
    
    pub fn reset_peaks(&mut self) {
        self.peak_cpu = 0.0;
        self.peak_vram_pct = 0.0;
        self.peak_gpu_pct = 0.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
    Stable,
}

#[derive(Debug, Clone)]
pub struct SmoothedMetrics {
    pub metrics: Metrics,
    pub trend: Trend,
    pub peak_cpu: f32,
    pub peak_vram_pct: f32,
    pub peak_gpu_pct: f32,
}
```

- [ ] **Step 2: 修改 metrics.rs 集成滑动平均**

在 `src/server/metrics.rs` 中修改 `emit_metrics` 函数，使用 `MetricsSmoother` 进行平滑处理。

- [ ] **Step 3: 在前端显示趋势箭头和峰值**

修改 `dist/main.js` 中的指标更新逻辑，添加趋势箭头显示和峰值显示。

- [ ] **Step 4: 添加「重置峰值」按钮**

在 `dist/index.html` 监控面板中添加重置按钮。

- [ ] **Step 5: 测试**

```bash
cargo test --lib metrics
cargo build --release
```

---

## Task 5: 集成测试与文档

- [ ] **Step 1: 编写集成测试**

为所有新功能编写集成测试，覆盖：
- 日志导出（txt/json/csv 格式）
- 配置备份创建、列出、恢复、删除
- 错误诊断
- 滑动平均计算

- [ ] **Step 2: 更新 README.md**

在 README 中添加新功能的使用说明。

- [ ] **Step 3: 更新 CHANGELOG.md**

记录 v0.4.0 的新功能。

- [ ] **Step 4: 完整构建测试**

```bash
cargo test --lib
cargo clippy --all-targets --release
cargo build --release
```

---

## 验证清单

实现完成后，检查以下项目：

- [ ] 所有新功能都有单元测试覆盖
- [ ] `cargo test --lib` 全部通过
- [ ] `cargo clippy --all-targets --release` 0 warning
- [ ] IPC 命令名与前端调用一致
- [ ] 前端 UI 在亮/暗主题下都正常显示
- [ ] 日志导出功能能正确生成 txt/json/csv 文件
- [ ] 配置备份能正确创建、列出、恢复、删除
- [ ] 错误诊断能检测出常见问题
- [ ] 性能监控显示趋势箭头和峰值
- [ ] 没有引入新的 clippy warning
- [ ] 没有破坏现有功能

---

**Plan complete.** Two execution options:

1. **Subagent-Driven (recommended)** - 为每个 Task 分配独立 subagent，任务间进行审查
2. **Inline Execution** - 在当前会话中使用 executing-plans 批量执行

Which approach?
