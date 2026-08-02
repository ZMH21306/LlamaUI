//! 自动更新检查。
//!
//! 通过 GitHub Releases API 检查最新版本，识别新版本目录，
//! 检测旧版本残留并提示用户清理。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{UNIX_EPOCH, Duration};

/// 更新检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    /// 是否有新版本
    pub update_available: bool,
    /// 最新版本号（如 "v0.4.0"）
    pub latest_version: String,
    /// 当前版本号
    pub current_version: String,
    /// 下载链接
    pub download_url: String,
    /// 发布说明
    pub release_notes: String,
    /// 旧版本残留目录列表
    pub old_installations: Vec<OldInstallation>,
}

/// 旧版本安装信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OldInstallation {
    /// 安装路径
    pub path: String,
    /// 版本号（从路径或文件推断）
    pub version: String,
    /// 最后修改时间
    pub last_modified: u64,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_RELEASES_API: &str = "https://api.github.com/repos/ZMH21306/LlamaUI/releases/latest";

/// 检查更新
pub fn check_for_updates() -> anyhow::Result<UpdateCheckResult> {
    // 1. 获取最新版本信息
    let latest = fetch_latest_release()?;
    
    // 2. 解析版本比较
    let update_available = is_newer_version(&latest.tag_name, CURRENT_VERSION);
    
    // 3. 检测旧版本残留
    let old_installations = detect_old_installations()?;
    
    Ok(UpdateCheckResult {
        update_available,
        latest_version: latest.tag_name.clone(),
        current_version: format!("v{}", CURRENT_VERSION),
        download_url: latest.html_url,
        release_notes: latest.body,
        old_installations,
    })
}

/// GitHub Release API 响应
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: String,
}

fn fetch_latest_release() -> anyhow::Result<GitHubRelease> {
    // 使用 ureq 进行简单的 HTTP GET（无外部依赖）
    // 注意：实际项目中应该使用 reqwest 或更成熟的 HTTP 客户端
    let response = ureq::get(GITHUB_RELEASES_API)
        .set("User-Agent", "LlamaUI")
        .call()
        .map_err(|e| anyhow::anyhow!("网络请求失败：{}", e))?;
    
    let release: GitHubRelease = response
        .into_json()
        .map_err(|e| anyhow::anyhow!("解析响应失败：{}", e))?;
    
    Ok(release)
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    // 简化版本比较：去除 "v" 前缀后按语义化版本比较
    let latest_clean = latest.strip_prefix('v').unwrap_or(latest);
    let current_clean = current.strip_prefix('v').unwrap_or(current);
    
    let latest_parts: Vec<u32> = latest_clean
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    let current_parts: Vec<u32> = current_clean
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    
    for i in 0..3 {
        let l = latest_parts.get(i).copied().unwrap_or(0);
        let c = current_parts.get(i).copied().unwrap_or(0);
        if l > c {
            return true;
        } else if l < c {
            return false;
        }
    }
    false
}

/// 检测旧版本残留
fn detect_old_installations() -> anyhow::Result<Vec<OldInstallation>> {
    let mut old_installations = Vec::new();
    
    // 检测常见的安装位置
    let search_dirs = get_search_directories();
    
    for dir in search_dirs {
        if dir.exists() {
            if let Some(installation) = check_directory(&dir) {
                // 只添加与当前版本不同的安装
                if installation.version != format!("v{}", CURRENT_VERSION) {
                    old_installations.push(installation);
                }
            }
        }
    }
    
    Ok(old_installations)
}

fn get_search_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    
    // Program Files
    if let Ok(program_files) = std::env::var("PROGRAMFILES") {
        dirs.push(PathBuf::from(program_files).join("LlamaUI"));
    }
    if let Ok(program_files_x86) = std::env::var("PROGRAMFILES(X86)") {
        dirs.push(PathBuf::from(program_files_x86).join("LlamaUI"));
    }
    
    // Local AppData
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(&local_appdata).join("Programs").join("LlamaUI"));
        dirs.push(PathBuf::from(&local_appdata).join("LlamaUI"));
    }
    
    // 用户主目录
    if let Ok(home) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(&home).join("AppData").join("Local").join("LlamaUI"));
        dirs.push(PathBuf::from(&home).join("LlamaUI"));
    }
    
    dirs
}

fn check_directory(path: &Path) -> Option<OldInstallation> {
    // 检查目录是否存在版本标识文件
    let version_file = path.join("version.txt");
    let version = if version_file.exists() {
        fs::read_to_string(&version_file).ok()?
    } else {
        // 尝试从目录名推断版本
        path.file_name()?
            .to_string_lossy()
            .to_string()
    };
    
    // 获取最后修改时间
    let metadata = fs::metadata(path).ok()?;
    let last_modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    
    Some(OldInstallation {
        path: path.to_string_lossy().to_string(),
        version,
        last_modified,
    })
}

/// 清理旧版本
pub fn cleanup_old_installation(path: &str) -> anyhow::Result<()> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(());
    }
    
    // 删除目录（递归）
    fs::remove_dir_all(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer_version("v0.4.0", "v0.3.0"));
        assert!(is_newer_version("v1.0.0", "v0.9.9"));
        assert!(is_newer_version("v0.3.1", "v0.3.0"));
        assert!(!is_newer_version("v0.3.0", "v0.3.0"));
        assert!(!is_newer_version("v0.2.0", "v0.3.0"));
    }

    #[test]
    fn version_comparison_without_v_prefix() {
        assert!(is_newer_version("0.4.0", "0.3.0"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn update_check_result_serialization() {
        let result = UpdateCheckResult {
            update_available: true,
            latest_version: "v0.4.0".to_string(),
            current_version: "v0.3.0".to_string(),
            download_url: "https://github.com/ZMH21306/LlamaUI/releases/tag/v0.4.0".to_string(),
            release_notes: "New features".to_string(),
            old_installations: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"update_available\":true"));
        assert!(json.contains("\"latest_version\":\"v0.4.0\""));
    }

    #[test]
    fn old_installation_serialization() {
        let installation = OldInstallation {
            path: "C:\\Program Files\\LlamaUI".to_string(),
            version: "v0.2.0".to_string(),
            last_modified: 1234567890,
        };
        let json = serde_json::to_string(&installation).unwrap();
        assert!(json.contains("\"path\":\"C:\\\\Program Files\\\\LlamaUI\""));
        assert!(json.contains("\"version\":\"v0.2.0\""));
    }
}
