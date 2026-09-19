//! 自动更新检查模块。
//!
//! 通过 GitHub Releases API 检查最新版本，识别新版本目录，检测旧版本残留并提示用户清理。
//!
//! - [`check`]：`check_for_updates` / `cleanup_old_installation` / `UpdateCheckResult` / `OldInstallation`

pub mod check;

pub use check::{
    check_for_updates, cleanup_old_installation, is_newer_version, get_platform,
    OldInstallation, UpdateCheckResult,
};