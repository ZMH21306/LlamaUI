//! 自动更新模块。
//!
//! 通过 GitHub Releases API 检查最新版本，识别新版本目录，检测旧版本残留并提示用户清理。
//!
//! - [`check`]：`check_for_updates` / `cleanup_old_installation` / `UpdateCheckResult` / `OldInstallation`
//! - [`download`]：`download_update` — 下载更新包并推送进度事件

use std::sync::atomic::AtomicBool;

pub static UPDATE_DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);

pub mod check;
pub mod download;

pub use check::{
    check_for_updates, cleanup_old_installation, is_newer_version, get_platform,
    OldInstallation, UpdateCheckResult,
};
pub use download::{download_update};