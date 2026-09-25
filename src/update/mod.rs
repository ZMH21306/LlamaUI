//! 自动更新模块。
//!
//! 通过静态 Manifest JSON 文件检查最新版本，识别新版本目录，检测旧版本残留并提示用户清理。
//! 替代 GitHub Releases API，支持任意 HTTP 服务器作为更新源。
//!
//! - [`check`]：`check_for_updates` / `cleanup_old_installation` / `UpdateCheckResult` / `OldInstallation`
//! - [`download`]：`download_update` — 下载更新包并推送进度事件
//! - [`manifest`]：`ManifestClient` — Manifest JSON 解析与 HTTP 请求

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::LazyLock;
use std::sync::Mutex;

use tokio::sync::watch;

/// P0-9: 替换单例 AtomicBool，支持多下载任务独立取消。
/// 使用 `Mutex<HashMap<download_id, watch::Sender<bool>>>` 管理每个下载的取消信号。
/// 保留原有静态 AtomicBool 以兼容现有 IPC 调用（单下载场景）。
pub static UPDATE_DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);

/// 多下载任务取消信号映射表：`download_id -> watch::Sender<bool>`
/// 新代码应使用此结构，逐步替代 UPDATE_DOWNLOAD_CANCEL。
pub static UPDATE_DOWNLOAD_CANCELS: LazyLock<Mutex<HashMap<String, watch::Sender<bool>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// 创建一个新的取消接收器，返回 (download_id, cancel_rx)
/// download_id 为 UUID，用于后续取消特定下载任务。
pub fn create_update_download_cancel() -> (String, watch::Receiver<bool>) {
    let (tx, rx) = watch::channel(false);
    let download_id = uuid::Uuid::new_v4().to_string();
    UPDATE_DOWNLOAD_CANCELS.lock().unwrap().insert(download_id.clone(), tx);
    (download_id, rx)
}

/// 移除指定 download_id 的取消信号（下载完成/失败/取消后调用）。
pub fn remove_update_download_cancel(download_id: &str) {
    UPDATE_DOWNLOAD_CANCELS.lock().unwrap().remove(download_id);
}

pub mod check;
pub mod download;
pub mod install;
pub mod manifest;

pub use check::{
    check_for_updates, cleanup_old_installation, is_newer_version, get_platform,
    OldInstallation, UpdateCheckResult,
};
pub use download::download_update;
pub use install::install_update;