//! 更新检查模块。
//!
//! 通过 [`check_for_updates`] 获取 Manifest 并比较版本号，
//! 识别旧版本安装目录并提示用户清理。

use serde::{Deserialize, Serialize};
use std::fs;

use crate::update::error::UpdateError;

use super::manifest::ManifestClient;

/// 更新检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    /// 是否有新版本可用
    pub update_available: bool,
    /// 最新版本号（带 v 前缀）
    pub latest_version: String,
    /// 当前版本号（带 v 前缀）
    pub current_version: String,
    /// 下载 URL
    pub download_url: String,
    /// 发布说明
    pub release_notes: String,
    /// 旧版本安装记录
    pub old_installations: Vec<OldInstallation>,
    /// 当前平台标识
    pub platform: String,
    /// 预期文件大小（字节）
    pub file_size: u64,
    /// 期望的 SHA256 校验和
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Manifest 签名是否通过验证
    #[serde(default)]
    pub signature_verified: bool,
}

/// 旧版本安装记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OldInstallation {
    /// 安装目录路径
    pub path: String,
    /// 版本号
    pub version: String,
    /// 最后修改时间（Unix 时间戳）
    pub last_modified: u64,
}

/// 异步检查更新。
pub async fn check_for_updates() -> Result<UpdateCheckResult, UpdateError> {
    let current_version = env!("CARGO_PKG_VERSION");
    let current_version_tag = format!("v{}", current_version);
    tracing::info!(target: "UpdateCheck", current_version = %current_version_tag, "start update check");

    // 创建 Manifest 客户端
    let client = ManifestClient::new().map_err(UpdateError::Network)?;

    // 获取 Manifest
    let manifest = client.fetch().await.map_err(UpdateError::Network)?;

    // 验证版本号格式
    if !is_valid_version_format(&manifest.latest_version) {
        return Err(UpdateError::InvalidVersion(manifest.latest_version.clone()));
    }

    let latest_tag = manifest.latest_version.clone();
    let is_newer = is_newer_version(&latest_tag, current_version);
    tracing::info!(
        target: "UpdateCheck",
        latest = %latest_tag,
        current = %current_version_tag,
        update_available = is_newer,
        "version compare done"
    );

    // 检测旧版本安装目录
    let old_installations = detect_old_installations(current_version);
    if !old_installations.is_empty() {
        tracing::info!(target: "UpdateCheck", count = old_installations.len(), "old installations found");
    }

    // 获取平台信息
    let platform = get_platform();

    // 查找匹配平台的资产
    let asset = manifest.assets.get_asset(&platform).cloned();
    let (download_url, file_size, sha256) = match asset {
        Some(a) => (a.url, a.size, a.sha256),
        None => {
            tracing::warn!(target: "UpdateCheck", platform = %platform, "no matching asset");
            (String::new(), 0, None)
        }
    };

    // 验证 Manifest 签名（若提供）
    let signature_verified = match &manifest.signature {
        Some(sig) => match verify_manifest_signature(&manifest, sig) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(target: "UpdateCheck", error = %e, "signature verification failed");
                false
            }
        },
        None => {
            tracing::warn!(target: "UpdateCheck", "manifest has no signature");
            false
        }
    };

    Ok(UpdateCheckResult {
        update_available: is_newer,
        latest_version: latest_tag,
        current_version: current_version_tag,
        download_url,
        release_notes: manifest.release_notes.unwrap_or_default(),
        old_installations,
        platform,
        file_size,
        sha256,
        signature_verified,
    })
}

/// 验证 Manifest 签名（Ed25519）。
fn verify_manifest_signature(
    manifest: &super::manifest::UpdateManifest,
    signature: &str,
) -> Result<(), String> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    // 解析公钥（Base64）—— 优先从环境变量读取，回退到内置常量
    let env_pubkey = std::env::var("MANIFEST_PUBLIC_KEY_BASE64")
        .unwrap_or_else(|_| super::manifest::MANIFEST_PUBLIC_KEY_BASE64.to_string());
    let pubkey_bytes = STANDARD
        .decode(&env_pubkey)
        .map_err(|e| format!("解析公钥失败：{}", e))?;
    if pubkey_bytes.len() != 32 {
        return Err(format!("公钥长度为 {}，期望 32", pubkey_bytes.len()));
    }
    let pubkey_array: [u8; 32] = pubkey_bytes
        .try_into()
        .map_err(|_| "公钥长度错误".to_string())?;
    let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|e| format!("公钥格式错误：{}", e))?;

    // 解析签名（Base64）
    let sig_bytes = STANDARD
        .decode(signature)
        .map_err(|e| format!("解析签名失败：{}", e))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| format!("签名格式错误：{}", e))?;

    // 对 Manifest 的 JSON 序列化字节进行验证
    let manifest_json =
        serde_json::to_vec(manifest).map_err(|e| format!("序列化 Manifest 失败：{}", e))?;

    verifying_key
        .verify(&manifest_json, &signature)
        .map_err(|e| format!("签名验证失败：{}", e))?;

    Ok(())
}

/// 判断 `latest` 是否比 `current` 更新。
///
/// 支持 semver 预发布标识：
/// - `v1.0.0-rc1` < `v1.0.0`（预发布小于正式版）
/// - `v1.0.0-alpha` < `v1.0.0-rc1`（alpha < rc）
/// - `v1.0.0` = `v1.0.0`（相等）
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    use std::cmp::Ordering;

    let lc = latest.trim_start_matches('v');
    let cc = current.trim_start_matches('v');

    let (l_main, l_pre) = split_pre_release(lc);
    let (c_main, c_pre) = split_pre_release(cc);

    let lp: Vec<u32> = l_main.split('.').filter_map(|s| s.parse().ok()).collect();
    let cp: Vec<u32> = c_main.split('.').filter_map(|s| s.parse().ok()).collect();

    for i in 0..3 {
        let l = lp.get(i).copied().unwrap_or(0);
        let c = cp.get(i).copied().unwrap_or(0);
        if l > c {
            return true;
        }
        if l < c {
            return false;
        }
    }

    // 主版本号相等，比较预发布标识
    match (l_pre, c_pre) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(l), Some(c)) => l.cmp(c) == Ordering::Greater,
    }
}

/// 拆分版本字符串为 (主版本，预发布标识)。
fn split_pre_release(version: &str) -> (&str, Option<&str>) {
    match version.find('-') {
        Some(idx) => (&version[..idx], Some(&version[idx + 1..])),
        None => (version, None),
    }
}

/// 获取当前平台标识（如 `windows-x64`）。
pub fn get_platform() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let a = match arch {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        _ => arch,
    };
    format!("{}-{}", os, a)
}

/// 扫描旧版本安装目录。
fn detect_old_installations(current_version: &str) -> Vec<OldInstallation> {
    let mut installs = Vec::new();
    let dirs = get_search_dirs();
    for dir in dirs {
        if dir.exists() && dir.is_dir() {
            scan_for_old_installations(&dir, current_version, &mut installs);
        }
    }
    installs
}

/// 获取搜索目录。
fn get_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = dirs::home_dir() {
        dirs.push(h.join(".llamaui"));
    }
    match std::env::consts::OS {
        "windows" => {
            if let Ok(p) = std::env::var("ProgramFiles") {
                dirs.push(std::path::PathBuf::from(p).join(".llamaui"));
            }
            if let Ok(p) = std::env::var("LocalAppData") {
                dirs.push(std::path::PathBuf::from(p).join("llamaui"));
            }
        }
        "macos" => {
            dirs.push(std::path::PathBuf::from("/Applications/llamaui.app"));
        }
        _ => {
            dirs.push(std::path::PathBuf::from("/opt/llamaui"));
        }
    }
    dirs
}

/// 递归扫描目录中的旧版本。
fn scan_for_old_installations(
    root: &std::path::Path,
    current_version: &str,
    installs: &mut Vec<OldInstallation>,
) {
    let entries = fs::read_dir(root).into_iter().flatten();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name_str = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let dir_version = extract_version_from_name(name_str);
        if let Some(ref ver) = dir_version {
            if ver != current_version
                && !installs.iter().any(|i| i.path == path.to_string_lossy())
            {
                let last_modified = fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                installs.push(OldInstallation {
                    path: path.to_string_lossy().to_string(),
                    version: ver.clone(),
                    last_modified,
                });
            }
        }
    }
}

/// 清理旧版本安装。
pub fn cleanup_old_installation(path: &str) -> Result<(), UpdateError> {
    let path = std::path::PathBuf::from(path);
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(UpdateError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Path is not a directory",
        )));
    }

    let ps = path.to_string_lossy().to_lowercase();
    if !ps.contains("llamaui") && !ps.contains("llama-ui") {
        return Err(UpdateError::Other("拒绝删除非 llamaui 路径".to_string()));
    }
    for dp in &[
        "C:\\Windows",
        "C:\\Program Files",
        "C:\\Program Files (x86)",
        "/usr",
        "/bin",
        "/sbin",
        "/etc",
        "/System",
    ] {
        if ps.starts_with(&dp.to_lowercase()) {
            return Err(UpdateError::Other("拒绝删除系统目录".to_string()));
        }
    }

    std::fs::remove_dir_all(&path).map_err(UpdateError::Io)?;
    Ok(())
}

/// 验证版本号格式是否符合规范。
fn is_valid_version_format(version: &str) -> bool {
    let version = version.strip_prefix('v').unwrap_or(version);
    let parts: Vec<&str> = version.split('-').collect();
    let main_version = parts[0];
    let version_parts: Vec<&str> = main_version.split('.').collect();
    if version_parts.len() != 3 {
        return false;
    }
    for part in &version_parts {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    if parts.len() > 1 {
        let prerelease = parts[1];
        if !prerelease.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
            return false;
        }
    }
    true
}

/// 从文件名中提取版本号。
fn extract_version_from_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'v' || bytes[i] == b'V' || bytes[i].is_ascii_digit() {
            let start = i;
            if bytes[i] == b'v' || bytes[i] == b'V' {
                i += 1;
            }
            let mut has_digit = false;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                has_digit = true;
                i += 1;
            }
            if !has_digit || i >= bytes.len() || bytes[i] != b'.' {
                i += 1;
                continue;
            }
            i += 1;
            let minor_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == minor_start || i >= bytes.len() || bytes[i] != b'.' {
                i += 1;
                continue;
            }
            i += 1;
            let patch_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == patch_start {
                i += 1;
                continue;
            }
            let version = &name[start..i];
            let version = if version.starts_with('v') || version.starts_with('V') {
                &version[1..]
            } else {
                version
            };
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
    fn test_version() {
        assert!(is_newer_version("v1.0.0", "0.9.0"));
        assert!(!is_newer_version("v0.3.0", "v0.4.0"));
        assert!(is_newer_version("v0.4.1", "v0.4.0"));
    }

    #[test]
    fn test_prerelease_comparison() {
        assert!(is_newer_version("v1.0.0", "1.0.0-rc1"));
        assert!(is_newer_version("v1.0.0-rc1", "0.9.0"));
        assert!(is_newer_version("v1.0.0-rc2", "1.0.0-rc1"));
        assert!(!is_newer_version("v1.0.0-rc1", "1.0.0-rc2"));
        assert!(!is_newer_version("v1.0.0", "1.0.0"));
    }

    #[test]
    fn test_platform() {
        let p = get_platform();
        assert!(!p.is_empty());
        assert!(p.contains('-'));
    }

    #[test]
    fn test_extract() {
        assert_eq!(
            extract_version_from_name("llama-ui-v0.3.0"),
            Some("0.3.0".to_string())
        );
        assert_eq!(extract_version_from_name("random"), None);
    }

    #[test]
    fn test_split_pre_release() {
        assert_eq!(split_pre_release("1.0.0"), ("1.0.0", None));
        assert_eq!(split_pre_release("1.0.0-rc1"), ("1.0.0", Some("rc1")));
        assert_eq!(
            split_pre_release("1.0.0-alpha.1"),
            ("1.0.0", Some("alpha.1"))
        );
    }

    #[test]
    fn test_update_check_result_serialization() {
        let result = UpdateCheckResult {
            update_available: true,
            latest_version: "v0.4.0".to_string(),
            current_version: "v0.3.0".to_string(),
            download_url: "https://example.com/download".to_string(),
            release_notes: "New features".to_string(),
            old_installations: vec![],
            platform: "windows-x64".to_string(),
            file_size: 1024 * 1024 * 50,
            sha256: Some("abc123".to_string()),
            signature_verified: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: UpdateCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.update_available, true);
        assert_eq!(deserialized.latest_version, "v0.4.0");
        assert_eq!(deserialized.sha256, Some("abc123".to_string()));
        assert!(deserialized.signature_verified);
    }

    #[test]
    fn test_valid_version_format() {
        assert!(is_valid_version_format("v0.7.0"));
        assert!(is_valid_version_format("0.7.0"));
        assert!(is_valid_version_format("v1.2.3-rc1"));
        assert!(is_valid_version_format("v1.0.0-alpha.1"));
        assert!(!is_valid_version_format("invalid"));
        assert!(!is_valid_version_format("v1.0"));
        assert!(!is_valid_version_format("v1.0.0.0"));
    }

    #[test]
    fn test_platform_detection() {
        let platform = get_platform();
        assert!(platform.contains('-'));
        let parts: Vec<&str> = platform.split('-').collect();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_extract_version_edge_cases() {
        assert_eq!(
            extract_version_from_name("llama-ui-v0.3.0"),
            Some("0.3.0".to_string())
        );
        assert_eq!(extract_version_from_name("random"), None);
        assert_eq!(extract_version_from_name("v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(
            extract_version_from_name("LlamaUI-v0.7.0-windows-x64"),
            Some("0.7.0".to_string())
        );
    }
}
