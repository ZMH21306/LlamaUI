//! 日志发射统一入口。
//!
//! 所有 `emit_log` / `emit_step` / `emit_status` 都通过本模块的函数转发。
//! 这样：
//! - 前端收到的事件 payload 类型与 `events` 模块严格一致
//! - 未来要加批处理 / 节流 / 持久化只需改这一处
//! - 调用方代码不需要关心 Tauri 事件 API 细节

use tauri::{AppHandle, Emitter};

use crate::events::{
    LogLine, ServerStatus, StepStatus, EVT_SERVER_LOG, EVT_SERVER_STATUS, EVT_SERVER_STEP,
};

/// 发送一条普通日志行（无分组）。
///
/// `stream` 通常是 `"stdout"` / `"stderr"` / `"system"`。
pub fn emit_log(app: &AppHandle, stream: &str, text: &str) {
    emit_log_to(app, stream, text, None);
}

/// 发送一条带分组的日志行。
///
/// `group` 用于初始化步骤等可折叠日志组；`None` 表示普通日志。
pub fn emit_log_to(app: &AppHandle, stream: &str, text: &str, group: Option<&str>) {
    let line = LogLine {
        timestamp: crate::util::time::now_ts(),
        stream: stream.into(),
        text: text.into(),
        group: group.map(|s| s.to_string()),
    };
    let _ = app.emit(EVT_SERVER_LOG, &line);
}

/// 发送一个初始化步骤的状态变更事件。
///
/// `status` 通常是 `"pending"` / `"running"` / `"success"` / `"failed"`。
/// `auto_expand = true` 时前端会自动展开此步骤的日志组。
pub fn emit_step(app: &AppHandle, id: &str, name: &str, status: &str, auto_expand: bool) {
    let step = StepStatus {
        id: id.to_string(),
        name: name.to_string(),
        status: status.to_string(),
        auto_expand,
    };
    let _ = app.emit(EVT_SERVER_STEP, &step);
}

/// 发送服务状态变化事件。
pub fn emit_status(app: &AppHandle, status: ServerStatus) {
    let _ = app.emit(EVT_SERVER_STATUS, &status);
}
