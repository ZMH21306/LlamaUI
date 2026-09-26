//! 更新错误类型。
//!
//! 所有更新相关的错误都通过此枚举统一返回，与 `AppError` 体系对齐。
//! 前端可根据错误变体展示精确的用户提示。

use thiserror::Error;

/// 更新流程中的统一错误类型。
#[derive(Debug, Error)]
pub enum UpdateError {
    /// 网络请求失败（Manifest 下载 / 更新包下载）。
    #[error("网络错误：{0}")]
    Network(#[from] crate::net::NetError),

    /// Manifest JSON 解析失败。
    #[error("Manifest 解析失败：{0}")]
    ManifestParse(#[from] serde_json::Error),

    /// 版本号格式无效或无法比较。
    #[error("版本号无效：{0}")]
    InvalidVersion(String),

    /// 当前版本已是最新。
    #[error("已是最新版本（{0}）")]
    AlreadyUpToDate(String),

    /// 平台不受支持。
    #[error("当前平台不受支持：{0}")]
    UnsupportedPlatform(String),

    /// 更新包 SHA256 校验不匹配。
    #[error("SHA256 校验失败：期望 {expected}，实际 {actual}")]
    Sha256Mismatch { expected: String, actual: String },

    /// 更新包文件大小不匹配。
    #[error("文件大小不匹配：期望 {expected}，实际 {actual}")]
    SizeMismatch { expected: u64, actual: u64 },

    /// 更新包缺少必要文件。
    #[error("更新包不完整：{0}")]
    IncompletePackage(String),

    /// 文件系统错误（备份/替换/清理）。
    #[error("文件操作失败：{0}")]
    Io(#[from] std::io::Error),

    /// 签名验证失败（Manifest 被篡改）。
    #[error("Manifest 签名验证失败")]
    SignatureVerification,

    /// 更新被用户取消。
    #[error("更新已取消")]
    Cancelled,

    /// 下载超时。
    #[error("下载超时")]
    Timeout,

    /// 通用回退错误。
    #[error("更新失败：{0}")]
    Other(String),
}