use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use super::manifest::ManifestClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub update_available: bool,
    pub latest_version: String,
    pub current_version: String,
    pub download_url: String,
    pub release_notes: String,
    pub old_installations: Vec<OldInstallation>,
    pub platform: String,
    pub file_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OldInstallation {
    pub path: String,
    pub version: String,
    pub last_modified: u64,
}

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


pub fn check_for_updates() -> anyhow::Result<UpdateCheckResult> {
    let current_version = env!("CARGO_PKG_VERSION");
    let current_version_tag = format!("v{}", current_version);
    tracing::info!(target: "UpdateCheck", current_version = %current_version_tag, "start update check");
    let client = ManifestClient::new();
    let manifest = client.fetch()?;
    let latest_tag = manifest.latest_version.clone();
    let is_newer = is_newer_version(&latest_tag, current_version);
    tracing::info!(target: "UpdateCheck", latest = %latest_tag, current = %current_version_tag, update_available = is_newer, "version compare done");
    let old_installations = detect_old_installations(current_version);
    if !old_installations.is_empty() {
        tracing::info!(target: "UpdateCheck", count = old_installations.len(), "old installations found");
    }
    let platform = get_platform();
    let asset = client.get_asset_for_platform(&platform)?;
    let (download_url, file_size) = match asset {
        Some(a) => (a.url, a.size),
        None => {
            tracing::warn!(target: "UpdateCheck", platform = %platform, "no matching asset");
            (String::new(), 0)
        }
    };
    Ok(UpdateCheckResult {
        update_available: is_newer,
        latest_version: latest_tag,
        current_version: current_version_tag,
        download_url,
        release_notes: String::new(),
        old_installations,
        platform,
        file_size,
    })
}


pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let lc = latest.trim_start_matches('v');
    let cc = current.trim_start_matches('v');
    let lp: Vec<u32> = lc.split('.').filter_map(|s| s.parse().ok()).collect();
    let cp: Vec<u32> = cc.split('.').filter_map(|s| s.parse().ok()).collect();
    for i in 0..3 {
        let l = lp.get(i).copied().unwrap_or(0);
        let c = cp.get(i).copied().unwrap_or(0);
        if l > c { return true; }
        if l < c { return false; }
    }
    false
}

pub fn get_platform() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let a = match arch { "x86_64" => "x64", "aarch64" => "aarch64", "arm" => "arm", _ => arch };
    format!("{}-{}", os, a)
}


fn get_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = dirs::home_dir() { dirs.push(h.join(".llamaui")); }
    match std::env::consts::OS {
        "windows" => {
            if let Ok(p) = std::env::var("ProgramFiles") { dirs.push(std::path::PathBuf::from(p)); }
            if let Ok(p) = std::env::var("LOCALAPPDATA") { dirs.push(std::path::PathBuf::from(p)); }
            if let Some(h) = dirs::home_dir() { dirs.push(h); }
        }
        "linux" => {
            dirs.push(std::path::PathBuf::from("/usr/local/bin"));
            dirs.push(std::path::PathBuf::from("/opt"));
            if let Some(h) = dirs::home_dir() { dirs.push(h.join(".local")); }
        }
        "macos" => {
            dirs.push(std::path::PathBuf::from("/Applications"));
            if let Some(h) = dirs::home_dir() { dirs.push(h.join("Applications")); }
        }
        _ => {}
    }
    dirs
}


fn scan_for_old_installations(dir: &std::path::Path, current_version: &str, installs: &mut Vec<OldInstallation>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() { continue; }
        if let Some(name) = path.file_name() {
            let name_str = name.to_string_lossy().to_lowercase();
            if name_str.contains("llamaui") || name_str.contains("llama-ui") {
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
                        installs.push(OldInstallation {
                            path: path.to_string_lossy().to_string(),
                            version,
                            last_modified,
                        });
                    }
                }
                if let Some(ver) = extract_version_from_name(&name_str) {
                    if ver != current_version && !installs.iter().any(|i| i.path == path.to_string_lossy()) {
                        let last_modified = fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        installs.push(OldInstallation {
                            path: path.to_string_lossy().to_string(),
                            version: ver,
                            last_modified,
                        });
                    }
                }
            }
        }
    }
}


pub fn cleanup_old_installation(path: &str) -> anyhow::Result<()> {
    let path = std::path::PathBuf::from(path);
    if !path.exists() { return Ok(()); }
    let ps = path.to_string_lossy().to_lowercase();
    if !ps.contains("llamaui") && !ps.contains("llama-ui") {
        return Err(anyhow::anyhow!("拒绝删除非 llamaui 路径"));
    }
    for dp in &["C:\\Windows", "C:\\Program Files", "C:\\Program Files (x86)", "/usr", "/bin", "/sbin", "/etc", "/System"] {
        if ps.starts_with(&dp.to_lowercase()) {
            return Err(anyhow::anyhow!("拒绝删除系统目录"));
        }
    }
    fs::remove_dir_all(&path).map_err(|e| anyhow::anyhow!("删除失败: {}", e))
}


fn extract_version_from_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'v' || bytes[i] == b'V' {
            i += 1;
            let start = i;
            let mut has_digit = false;
            while i < bytes.len() && bytes[i].is_ascii_digit() { has_digit = true; i += 1; }
            if !has_digit || i >= bytes.len() || bytes[i] != b'.' { i += 1; continue; }
            i += 1;
            let minor_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            if i == minor_start || i >= bytes.len() || bytes[i] != b'.' { i += 1; continue; }
            i += 1;
            let patch_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
            if i == patch_start { i += 1; continue; }
            let version = &name[start..i];
            if version.contains('.') { return Some(version.to_string()); }
        }
        i += 1;
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_version() {
        assert!(is_newer_version("v1.0.0", "0.9.0"));
        assert!(!is_newer_version("v0.3.0", "v0.4.0"));
        assert!(is_newer_version("v0.4.1", "v0.4.0"));
    }
    #[test] fn test_platform() { let p = get_platform(); assert!(!p.is_empty()); assert!(p.contains('-')); }
    #[test] fn test_extract() { assert_eq!(extract_version_from_name("llama-ui-v0.3.0"), Some("0.3.0".to_string())); assert_eq!(extract_version_from_name("random"), None); }
}
﻿
