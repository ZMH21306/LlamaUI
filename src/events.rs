//! 应用级事件名常量 + 共享 payload 类型。
//!
//! # 设计动机
//!
//! 事件名散落在 `server::emit_log_to` / `detect::Ctx::emit` / `init::emit_step` 等
//! 多处地方，任何一处拼写错误都会变成「前端永远收不到消息」的隐性 bug。
//! 本模块把所有事件名集中管理，编译期就能发现不一致。
//!
//! # 兼容性
//!
//! 所有事件名字符串与现有前端 JS 代码**完全一致**（见 `dist/main.js` 的
//! `listen('server-log', ...)` 等），不允许改动。

use serde::{Deserialize, Serialize};

/// 服务状态变化事件。
#[allow(dead_code)] // 历史导出：crate 外部消费者可能依赖
pub const EVT_SERVER_STATUS: &str = "server-status";

/// 服务日志行事件（stdout / stderr / system）。
pub const EVT_SERVER_LOG: &str = "server-log";

/// 周期性指标事件（每 ~500ms 一次）。
#[allow(dead_code)] // 历史导出：crate 外部消费者可能依赖
pub const EVT_SERVER_METRICS: &str = "server-metrics";

/// 初始化步骤状态变化事件。
pub const EVT_SERVER_STEP: &str = "server-step";

/// 自动检测进度事件。
#[allow(dead_code)] // 历史导出：crate 外部消费者可能依赖
pub const EVT_DETECT_PROGRESS: &str = "detect-progress";

// ============================================================
// Payload 类型
// ============================================================

/// 服务生命周期状态。**这是前端可见的状态机**，新增/删除值需要同步前端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerStatus {
    /// 未运行。
    Stopped,
    /// 启动中（spawn 已发起但尚未稳定）。
    Starting,
    /// 运行中。
    Running,
    /// 异常退出。
    Crashed,
}

/// 单行日志。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// 格式 `YYYY-MM-DD HH:MM:SS.mmm`，按本地时区。
    pub timestamp: String,
    /// `"stdout"` / `"stderr"` / `"system"`。
    pub stream: String,
    /// 日志文本。已截断过长行。
    pub text: String,
    /// 可选：所属分组（用于初始化步骤等可折叠日志组）。
    /// `None` 表示普通日志，显示在「常规日志」组里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl LogLine {
    /// 构造一条普通日志行（无分组）。
    pub fn plain(stream: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            timestamp: crate::util::time::now_ts(),
            stream: stream.into(),
            text: text.into(),
            group: None,
        }
    }

    /// 构造一条属于特定分组的日志行。
    pub fn grouped(
        stream: impl Into<String>,
        text: impl Into<String>,
        group: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: crate::util::time::now_ts(),
            stream: stream.into(),
            text: text.into(),
            group: Some(group.into()),
        }
    }
}

/// 初始化步骤状态变更（前端根据 `auto_expand` 自动展开 / 折叠）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepStatus {
    /// 分组 ID，与 `LogLine.group` 对应。
    pub id: String,
    /// 显示名称（如 "① 环境检查"）。
    pub name: String,
    /// 状态：`"pending"` / `"running"` / `"success"` / `"failed"`。
    pub status: String,
    /// 是否自动展开；后端通知前端在收到此事件时强制展开。
    pub auto_expand: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_are_stable() {
        // 前端 dist/main.js 通过这些字符串 listen。
        // 改动需同步前端，否则前端收不到事件。
        assert_eq!(EVT_SERVER_STATUS, "server-status");
        assert_eq!(EVT_SERVER_LOG, "server-log");
        assert_eq!(EVT_SERVER_METRICS, "server-metrics");
        assert_eq!(EVT_SERVER_STEP, "server-step");
        assert_eq!(EVT_DETECT_PROGRESS, "detect-progress");
    }

    #[test]
    fn log_line_omits_none_group() {
        let line = LogLine::plain("stdout", "hello");
        let json = serde_json::to_string(&line).expect("序列化必须成功");
        assert!(!json.contains("group"), "None group 不应出现在 JSON 中");
    }

    #[test]
    fn log_line_includes_some_group() {
        let line = LogLine::grouped("system", "msg", "init");
        let json = serde_json::to_string(&line).expect("序列化必须成功");
        assert!(json.contains("\"group\":\"init\""));
    }
}
