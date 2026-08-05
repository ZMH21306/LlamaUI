//! 自动更新检查。
//!
//! 通过 curl 调用 GitHub Releases API 检查最新版本，识别新版本目录，
//! 检测旧版本残留并提示用户清理。使用 curl 子进程而非 ureq 库，
//! 以确保在各种网络环境（代理、TLS）下都能正常工作。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 获取 GitHub Token（优先级：GITHUB_TOKEN/gh token → GH_TOKEN → 无）
///
/// 认证后速率限制从 60 次/小时提升到 5000 次/小时（PAT）或 15000 次/小时（GitHub App）
fn get_github_token() -> Option<String> {
    // 1. 环境变量 GITHUB_TOKEN（可设为 GitHub App 或 PAT）
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            tracing::debug!(target: "UpdateCheck", source = "env:GITHUB_TOKEN", "获取到 token");
            return Some(token);
        }
    }
    // 2. gh CLI auth token（用户通过 gh auth login 登录后自动获取）
    if let Ok(output) = Command::new("gh").args(["auth", "token"]).output() {
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                tracing::debug!(target: "UpdateCheck", source = "gh-cli", "获取到 token");
                return Some(token);
            }
        }
    }
    // 3. 环境变量 GH_TOKEN（备选）
    if let Ok(token) = std::env::var("GH_TOKEN") {
        if !token.is_empty() {
            tracing::debug!(target: "UpdateCheck", source = "env:GH_TOKEN", "获取到 token");
            return Some(token);
        }
    }
    tracing::debug!(target: "UpdateCheck", "未找到认证 token，使用匿名请求（60 次/小时）");
    None
}

/// 用 curl 获取 URL 内容（自动继承系统代理和 TLS 配置）
fn curl_get_json(url: &str) -> anyhow::Result<String> {
    tracing::debug!(target: "UpdateCheck", url = %url, "curl GET 请求");

    let token = get_github_token();
    let auth_header = token.as_deref().map(|t| format!("Authorization: token {}", t));

    let mut args: Vec<&str> = vec![
        "-s", "-L", "--max-time", "15",
        "-H", "Accept: application/vnd.github.v3+json",
    ];
    // 需要持有 auth_header 的引用
    let auth_ref;
    if let Some(ref h) = auth_header {
        auth_ref = h.as_str();
        args.push("-H");
        args.push(auth_ref);
    }
    args.push(url);

    let output = Command::new("curl")
        .args(&args)
        .output()
        .map_err(|e| anyhow::anyhow!("curl 不存在或无法执行: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(target: "UpdateCheck", stderr = %stderr, "curl 请求失败");
        return Err(anyhow::anyhow!("curl 请求失败: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

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
    /// 运行平台信息（如 "windows-x64", "linux-aarch64"）
    pub platform: String,
}

/// 旧版本安装信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OldInstallation {
    /// 安装路径
    pub path: String,
    /// 版本号
    pub version: String,
    /// 最后修改时间（秒）
    pub last_modified: u64,
}

/// GitHub Release API 响应结构
#[derive(Debug, Deserialize)]
struct GitHubReleaseResponse {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GitHubAssetInfo>,
}

/// GitHub Release 资产信息
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GitHubAssetInfo {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// 主检查函数
#[allow(clippy::print_stderr)]
pub fn check_for_updates() -> anyhow::Result<UpdateCheckResult> {
    let current_version = env!("CARGO_PKG_VERSION");
    let current_version_tag = format!("v{}", current_version);

    tracing::info!(target: "UpdateCheck", current_version = %current_version_tag, "开始检查更新");

    // 1. 获取最新 Release
    let release = fetch_latest_release()?;
    let latest_tag = release.tag_name.clone();
    let is_newer = is_newer_version(&latest_tag, current_version);

    tracing::info!(target: "UpdateCheck",
        latest = %latest_tag,
        current = %current_version_tag,
        update_available = is_newer,
        "版本比较完成"
    );

    // 2. 检测旧版本
    let old_installations = detect_old_installations(current_version);
    if !old_installations.is_empty() {
        tracing::info!(target: "UpdateCheck", count = old_installations.len(), "发现旧版本残留");
    }

    // 3. 获取平台标识
    let platform = get_platform();

    Ok(UpdateCheckResult {
        update_available: is_newer,
        latest_version: latest_tag,
        current_version: current_version_tag,
        download_url: release.html_url,
        release_notes: release.body.unwrap_or_default(),
        old_installations,
        platform,
    })
}

/// 通过 curl 获取 GitHub Release 信息
fn fetch_latest_release() -> anyhow::Result<GitHubReleaseResponse> {
    let url = "https://api.github.com/repos/ZMH21306/LlamaUI/releases/latest";

    let json_str = curl_get_json(url)?;

    // 检查是否被限流
    if json_str.contains("API rate limit exceeded") {
        tracing::warn!(target: "UpdateCheck", "GitHub API 速率限制");
        return Err(anyhow::anyhow!("GitHub API 速率限制，请稍后再试"));
    }

    let release: GitHubReleaseResponse = serde_json::from_str(&json_str)
        .map_err(|e| anyhow::anyhow!("解析 Release 失败: {}", e))?;

    tracing::debug!(target: "UpdateCheck", tag = %release.tag_name, assets = release.assets.len(), "获取到 Release");

    Ok(release)
}

/// 版本比较：返回 latest 是否比 current 新
///
/// 支持格式: "v1.2.3" 或 "1.2.3"
fn is_newer_version(latest: &str, current: &str) -> bool {
    let latest_clean = latest.trim_start_matches('v');
    let current_clean = current.trim_start_matches('v');

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
        }
        if l < c {
            return false;
        }
    }

    false
}

/// 获取当前平台标识
pub fn get_platform() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let os_str = match os {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => os,
    };

    let arch_str = match arch {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        _ => arch,
    };

    format!("{}-{}", os_str, arch_str)
}

/// 检测可能的旧版本安装
fn detect_old_installations(current_version: &str) -> Vec<OldInstallation> {
    let mut installations = Vec::new();
    let search_dirs = get_search_dirs();

    for dir in &search_dirs {
        if dir.is_dir() {
            scan_for_old_installations(dir, current_version, &mut installations);
        }
    }

    installations
}

/// 获取搜索目录列表
fn get_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let os = std::env::consts::OS;

    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".llamaui"));
    }

    match os {
        "windows" => {
            if let Ok(program_files) = std::env::var("ProgramFiles") {
                dirs.push(PathBuf::from(program_files));
            }
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local_app_data));
            }
            if let Some(user_profile) = dirs::home_dir() {
                dirs.push(user_profile);
            }
        }
        "linux" => {
            dirs.push(PathBuf::from("/usr/local/bin"));
            dirs.push(PathBuf::from("/opt"));
            if let Some(home) = dirs::home_dir() {
                dirs.push(home.join(".local"));
            }
        }
        "macos" => {
            dirs.push(PathBuf::from("/Applications"));
            if let Some(home) = dirs::home_dir() {
                dirs.push(home.join("Applications"));
            }
        }
        _ => {}
    }

    dirs
}

/// 递归扫描目录，查找旧版本
fn scan_for_old_installations(
    dir: &Path,
    current_version: &str,
    installations: &mut Vec<OldInstallation>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        // 检查是否是 llamaui 相关目录
        if let Some(name) = path.file_name() {
            let name_str = name.to_string_lossy().to_lowercase();

            if name_str.contains("llamaui") || name_str.contains("llama-ui") {
                // 检查是否有 version.txt
                let version_file = path.join("version.txt");
                if let Ok(version) = fs::read_to_string(&version_file) {
                    let version = version.trim().to_string();
                    if !version.is_empty() && version != current_version {
                        let last_modified = fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);

                        tracing::debug!(target: "UpdateCheck",
                            path = %path.display(),
                            version = %version,
                            "发现旧版本"
                        );

                        installations.push(OldInstallation {
                            path: path.to_string_lossy().to_string(),
                            version,
                            last_modified,
                        });
                    }
                }

                // 从目录名推断版本
                if let Some(version) = extract_version_from_name(&name_str) {
                    if version != current_version {
                        let last_modified = fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);

                        // 避免重复
                        if !installations.iter().any(|i| i.path == path.to_string_lossy()) {
                            installations.push(OldInstallation {
                                path: path.to_string_lossy().to_string(),
                                version,
                                last_modified,
                            });
                        }
                    }
                }
            }
        }
    }
}

/// 清理旧版本安装目录
pub fn cleanup_old_installation(path: &str) -> anyhow::Result<()> {
    let path = PathBuf::from(path);

    if !path.exists() {
        tracing::debug!(target: "UpdateCheck", path = %path.display(), "路径不存在，跳过");
        return Ok(());
    }

    tracing::info!(target: "UpdateCheck", path = %path.display(), "清理旧版本目录");

    // 安全检查：只允许删除 llamaui 相关目录
    let path_str = path.to_string_lossy().to_lowercase();
    if !path_str.contains("llamaui") && !path_str.contains("llama-ui") {
        tracing::warn!(target: "UpdateCheck", path = %path.display(), "路径不包含 llamaui 关键词，拒绝删除");
        return Err(anyhow::anyhow!("安全拒绝：路径不包含 llamaui 关键词"));
    }

    // 安全检查：不允许删除系统关键目录
    let dangerous_paths = [
        "C:\\Windows", "C:\\Program Files", "C:\\Program Files (x86)",
        "/usr", "/bin", "/sbin", "/etc", "/System",
    ];
    for dp in &dangerous_paths {
        if path_str.starts_with(&dp.to_lowercase()) {
            tracing::warn!(target: "UpdateCheck", path = %path.display(), dangerous = dp, "拒绝删除系统目录");
            return Err(anyhow::anyhow!("安全拒绝：不允许删除系统目录 {}", dp));
        }
    }

    fs::remove_dir_all(&path)
        .map_err(|e| anyhow::anyhow!("删除目录失败: {}", e))?;

    tracing::info!(target: "UpdateCheck", path = %path.display(), "旧版本目录已清理");
    Ok(())
}

/// 从目录名提取版本号（手动解析，避免正则依赖）
fn extract_version_from_name(name: &str) -> Option<String> {
    // 查找 "v" 后跟数字序列
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'v' || bytes[i] == b'V' {
            i += 1;
            // 尝试解析 major.minor.patch
            let start = i;
            let mut has_digit = false;
            // major
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                has_digit = true;
                i += 1;
            }
            if !has_digit || i >= bytes.len() || bytes[i] != b'.' {
                continue;
            }
            i += 1; // skip '.'
            // minor
            let minor_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == minor_start || i >= bytes.len() || bytes[i] != b'.' {
                continue;
            }
            i += 1; // skip '.'
            // patch
            let patch_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == patch_start {
                continue;
            }
            let version = &name[start..i];
            // 基本校验：至少包含一个点
            if version.contains('.') {
                return Some(version.to_string());
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(is_newer_version("v1.0.0", "0.9.0"));
        assert!(is_newer_version("1.0.0", "0.9.0"));
        assert!(is_newer_version("v0.4.0", "v0.3.0"));
        assert!(!is_newer_version("v0.3.0", "v0.4.0"));
        assert!(!is_newer_version("v0.4.0", "v0.4.0"));
        assert!(is_newer_version("v1.0.0", "0.9.9"));
        assert!(is_newer_version("v0.4.1", "v0.4.0"));
        assert!(!is_newer_version("v0.4.0", "v0.4.1"));
    }

    #[test]
    fn test_platform_detection() {
        let platform = get_platform();
        assert!(!platform.is_empty());
        assert!(platform.contains('-'));
    }

    #[test]
    fn test_extract_version() {
        assert_eq!(
            extract_version_from_name("llama-ui-v0.3.0"),
            Some("0.3.0".to_string())
        );
        assert_eq!(
            extract_version_from_name("llamaui-v0.2.1-beta"),
            Some("0.2.1".to_string())
        );
        assert_eq!(extract_version_from_name("random"), None);
    }
}
