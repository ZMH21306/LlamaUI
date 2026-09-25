//! 自动更新模块。
//!
//! 通过静态 Manifest JSON 文件检查最新版本，识别新版本目录，检测旧版本残留并提示用户清理。
//! 替代 GitHub Releases API，支持任意 HTTP 服务器作为更新源。
//!
//! - [`check`]：`check_for_updates` / `cleanup_old_installation` / `UpdateCheckResult` / `OldInstallation`
//! - [`download`]：`download_update` — 下载更新包并推送进度事件
//! - [`manifest`]：`ManifestClient` — Manifest JSON 解析与 HTTP 请求

use std::sync::atomic::AtomicBool;

pub static UPDATE_DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);

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