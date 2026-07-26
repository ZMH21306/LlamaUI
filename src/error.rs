//! 应用级统一错误类型。
//!
//! # 设计目标
//!
//! - **可分层的错误**：`AppError` 是顶层枚举，子错误（`ConfigError` / `ProcessError` /
//!   `DetectError`）按职责域组织。调用方可以 `match` 处理特定子类型，也可以
//!   `?` 直接上抛。
//! - **用户可读**：`Display` 实现给前端直接显示（前端拿到的是 `to_string()`）。
//! - **保留来源**：`#[from]` 覆盖常见 std / 外部错误，让 `?` 自动转换。
//!
//! # 兼容性
//!
//! `AppError::to_string()` 与现有 `anyhow::Error::to_string()` 的中文消息**保持一致**，
//! 避免前端看到行为变化。

use std::path::PathBuf;
use thiserror::Error;

/// 顶层应用错误。各子模块错误通过 `#[from]` 自动转换。
#[derive(Debug, Error)]
pub enum AppError {
    /// 配置相关错误（验证、加载、迁移、路径不存在等）。
    #[error("{0}")]
    Config(#[from] ConfigError),

    /// 进程管理错误（启动失败、停止失败、命令解析失败等）。
    #[error("{0}")]
    Process(#[from] ProcessError),

    /// 自动检测错误（仅在检测流程自身异常时使用；"未找到"不是错误）。
    #[error("{0}")]
    Detect(#[from] DetectError),

    /// 标准 I/O 错误（文件读写、进程派生等）。
    #[error("I/O 错误：{0}")]
    Io(#[from] std::io::Error),

    /// JSON 序列化 / 反序列化错误。
    #[error("序列化错误：{0}")]
    Serde(#[from] serde_json::Error),

    /// 其它未分类错误（保留兜底通道，方便迁移期使用）。
    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// 构造一个未分类错误。等价于 `anyhow::anyhow!` 的轻量替代。
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Other(s.into())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e.to_string())
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

// ============================================================
// 子错误类型
// ============================================================

/// 配置错误。
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("端口号不能为 0")]
    PortZero,

    #[error("端口号越界：{0}（范围 1 ~ 65535）")]
    PortOutOfRange(u16),

    #[error("参数模式非法：`{0}`（必须是 normal/advanced/pro）")]
    InvalidMode(String),

    #[error("上下文大小越界：`{value}`（范围 128 ~ 1048576）")]
    CtxSizeOutOfRange { value: u32 },

    #[error("GPU 卸载层数越界：`{value}`（范围 -1 ~ 200）")]
    GpuLayersOutOfRange { value: i32 },

    #[error("MTP 草稿数量越界：`{value}`（范围 0 ~ 16）")]
    MtpDraftOutOfRange { value: u32 },

    #[error("路径中不能含 NUL 字符：`{field}`")]
    NulInPath { field: &'static str },

    #[error("路径不存在：`{0}`")]
    PathNotFound(PathBuf),

    #[error("路径不是文件：`{0}`")]
    NotAFile(PathBuf),

    #[error("路径不是目录：`{0}`")]
    NotADirectory(PathBuf),
}

/// 进程管理错误。
#[derive(Debug, Error)]
pub enum ProcessError {
    /// 服务已经在启动或运行中。
    #[error("服务已经在启动或运行中")]
    AlreadyRunning,

    /// 启动 llama-server 失败（spawn 错误）。
    #[error("启动 llama-server 失败：{0}")]
    SpawnFailed(String),

    /// 用户配置非法。
    #[error("配置非法：{0}")]
    InvalidConfig(String),

    /// 专业模式命令解析失败。
    #[error("专业模式命令解析失败：{0}")]
    BadProCommand(String),

    /// 模型目录未配置。
    #[error("请先在左侧填写模型目录（包含 .gguf 文件的文件夹）")]
    ModelsDirNotSet,

    /// 端口选择失败。
    #[error("端口选择失败：{0}")]
    PortSelection(String),

    /// 通用 IO 错误（spawn 失败、pipe 关闭等）。
    #[error("进程 I/O 错误：{0}")]
    Io(#[from] std::io::Error),
}

/// 自动检测错误。仅在检测流程自身异常时使用；"未找到"以 `DetectResult::found = false`
/// 表达，**不**作为错误抛出。
#[derive(Debug, Error)]
pub enum DetectError {
    #[error("检测被取消")]
    Cancelled,

    #[error("检测超时")]
    Timeout,

    #[error("阶段 {stage} 超出时间预算")]
    StageBudgetExceeded { stage: u8 },

    #[error("检测 IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    //! 验证错误类型在 `?` 自动转换与 Display 输出方面的行为。
    use super::*;

    #[test]
    fn config_error_display_includes_field_name() {
        let e = ConfigError::NulInPath { field: "llama_server_path" };
        let s = e.to_string();
        assert!(s.contains("llama_server_path"), "必须包含字段名：{}", s);
    }

    #[test]
    fn app_error_from_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let app: AppError = io.into();
        assert!(matches!(app, AppError::Io(_)));
    }

    #[test]
    fn app_error_from_string() {
        let app: AppError = "test error".to_string().into();
        assert!(matches!(app, AppError::Other(_)));
        assert_eq!(app.to_string(), "test error");
    }

    #[test]
    fn process_error_already_running_message() {
        let e = ProcessError::AlreadyRunning;
        assert_eq!(e.to_string(), "服务已经在启动或运行中");
    }
}
