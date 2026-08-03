//! llama.cpp 二进制自动下载与安装。
//!
//! 从 GitHub Releases（ggerganov/llama.cpp）下载对应平台的 llama-server，
//! 支持 GPU 后端自动选择、SHA256 校验、解压和进度回调。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// 下载进度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub stage: String,
    pub progress: f64,
    pub downloaded: u64,
    pub total: u64,
    pub message: String,
}

/// 下载结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResult {
    pub success: bool,
    pub path: String,
    pub file_size: u64,
    pub sha256: String,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

/// GPU 后端类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    Cpu,
    Cuda12_4,
    Cuda13_3,
    Rocm,
    Vulkan,
    Metal,
}

impl GpuBackend {
    /// 从字符串解析
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "cuda" | "cuda12" | "cuda12_4" | "cuda-12.4" => GpuBackend::Cuda12_4,
            "cuda13" | "cuda13_3" | "cuda-13.3" => GpuBackend::Cuda13_3,
            "rocm" => GpuBackend::Rocm,
            "vulkan" => GpuBackend::Vulkan,
            "metal" => GpuBackend::Metal,
            _ => GpuBackend::Cpu,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GpuBackend::Cpu => "cpu",
            GpuBackend::Cuda12_4 => "cuda-12.4",
            GpuBackend::Cuda13_3 => "cuda-13.3",
            GpuBackend::Rocm => "rocm",
            GpuBackend::Vulkan => "vulkan",
            GpuBackend::Metal => "metal",
        }
    }
}

/// GitHub Release API 响应
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

/// GitHub Release 资产
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// 检测系统 GPU 后端
pub fn detect_gpu_backend() -> GpuBackend {
    let os = std::env::consts::OS;

    if os == "macos" {
        tracing::info!(target: "LlamaDownloader", backend = "metal", "检测到 macOS，使用 Metal 后端");
        return GpuBackend::Metal;
    }

    if os == "windows" || os == "linux" {
        // 检测 NVIDIA GPU
        if detect_nvidia_gpu() {
            // 检测 CUDA 版本以选择最佳版本
            let cuda_ver = detect_cuda_version();
            if let Some(ver) = cuda_ver {
                // 尝试解析主版本号
                if let Some(major) = ver.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
                    if major >= 13 {
                        tracing::info!(target: "LlamaDownloader", cuda_version = %ver, backend = "cuda-13.3", "检测到 CUDA 13+");
                        return GpuBackend::Cuda13_3;
                    }
                }
            }
            tracing::info!(target: "LlamaDownloader", backend = "cuda-12.4", "检测到 NVIDIA GPU，使用 CUDA 12.4 兼容模式");
            return GpuBackend::Cuda12_4;
        }

        // 检测 AMD GPU
        if detect_amd_gpu() {
            if os == "linux" {
                tracing::info!(target: "LlamaDownloader", backend = "rocm", "检测到 AMD GPU，使用 ROCm");
                return GpuBackend::Rocm;
            }
            tracing::info!(target: "LlamaDownloader", backend = "vulkan", "检测到 AMD GPU，使用 Vulkan");
            return GpuBackend::Vulkan;
        }

        // 无 GPU
        tracing::info!(target: "LlamaDownloader", backend = "cpu", "未检测到 GPU，使用 CPU 后端");
    }

    GpuBackend::Cpu
}

fn detect_nvidia_gpu() -> bool {
    Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

fn detect_amd_gpu() -> bool {
    let os = std::env::consts::OS;

    #[cfg(target_os = "linux")]
    if os == "linux" {
        if let Ok(output) = Command::new("lspci").arg("-nn").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let lower = line.to_lowercase();
                if lower.contains("amd") || lower.contains("radeon") {
                    return true;
                }
            }
        }
        return false;
    }

    #[cfg(target_os = "windows")]
    if os == "windows" {
        if let Ok(output) = Command::new("wmic")
            .args(["path", "win32_videocontroller", "get", "name"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let lower = line.to_lowercase();
                if lower.contains("amd") || lower.contains("radeon") {
                    return true;
                }
            }
        }
        return false;
    }

    false
}

/// 检测 CUDA 版本（从 nvidia-smi 输出）
fn detect_cuda_version() -> Option<String> {
    let output = Command::new("nvidia-smi").output().ok()?;
    if !output.status.success() { return None; }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("CUDA Version:") {
            let parts: Vec<&str> = line.split("CUDA Version:").collect();
            if parts.len() > 1 {
                let version = parts[1].trim().split_whitespace().next()?;
                return Some(version.to_string());
            }
        }
    }
    None
}

/// 构建下载资产名（匹配 llama.cpp 实际发布命名）
fn build_asset_name(tag: &str, backend: GpuBackend) -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let arch_str = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => arch,
    };

    let ext = if os == "windows" { "zip" } else { "tar.gz" };

    // llama.cpp 命名格式: llama-{tag}-bin-{os}[-{backend}].{ext}
    // 示例: llama-b10238-bin-win-cuda-12.4-x64.zip
    //        llama-b10238-bin-ubuntu-x64.tar.gz
    //        llama-b10238-bin-macos-arm64.tar.gz
    let os_str = match os {
        "windows" => "win",
        "linux" => "ubuntu",
        "macos" => "macos",
        _ => os,
    };

    let backend_part = match backend {
        GpuBackend::Cpu => {
            if os == "windows" { "-cpu".to_string() } else { String::new() }
        }
        GpuBackend::Cuda12_4 => "-cuda-12.4".to_string(),
        GpuBackend::Cuda13_3 => "-cuda-13.3".to_string(),
        GpuBackend::Rocm => {
            if os == "linux" { "-rocm-7.2".to_string() } else { "-hip-radeon".to_string() }
        }
        GpuBackend::Vulkan => "-vulkan".to_string(),
        GpuBackend::Metal => String::new(), // macOS 只有一个包
    };

    format!("llama-{}-bin-{}{}-{}.{}", tag, os_str, backend_part, arch_str, ext)
}

/// 从 GitHub Release 查找匹配的资产
fn find_asset<'a>(release: &'a GitHubRelease, asset_name: &str) -> Option<&'a GitHubAsset> {
    release.assets.iter().find(|a| a.name == asset_name)
}

/// 下载文件
pub fn download_file(
    url: &str,
    dest: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<u64> {
    tracing::info!(target: "LlamaDownloader", url = %url, "开始下载文件");

    let response = ureq::get(url)
        .set("User-Agent", "LlamaUI")
        .timeout(Duration::from_secs(600)) // 10 分钟超时（大文件）
        .call()
        .map_err(|e| anyhow::anyhow!("下载请求失败: {}", e))?;

    let total_size: u64 = response
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    tracing::info!(target: "LlamaDownloader", bytes = total_size, mb = total_size as f64 / 1048576.0, "文件大小");

    let mut reader = response.into_reader();
    let mut file = fs::File::create(dest)
        .map_err(|e| anyhow::anyhow!("创建文件失败: {}", e))?;

    let mut downloaded: u64 = 0;
    let mut buffer = vec![0u8; 65536]; // 64KB 缓冲区
    let mut hasher = Sha256::new();

    loop {
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|e| anyhow::anyhow!("读取数据失败: {}", e))?;

        if bytes_read == 0 { break; }

        std::io::Write::write_all(&mut file, &buffer[..bytes_read])
            .map_err(|e| anyhow::anyhow!("写入文件失败: {}", e))?;

        hasher.update(&buffer[..bytes_read]);
        downloaded += bytes_read as u64;

        if let Some(cb) = progress_callback {
            let progress = if total_size > 0 { downloaded as f64 / total_size as f64 } else { 0.0 };
            cb(DownloadProgress {
                stage: "downloading".to_string(),
                progress,
                downloaded,
                total: total_size,
                message: format!("下载中... {:.1} / {:.1} MB", downloaded as f64 / 1048576.0, total_size as f64 / 1048576.0),
            });
        }
    }

    let sha256_hex = format!("{:x}", hasher.finalize());
    tracing::info!(target: "LlamaDownloader", sha256 = %sha256_hex, "SHA256 计算完成");

    Ok(downloaded)
}

/// 解压 tar.gz
#[allow(clippy::print_stderr)]
pub fn extract_tar_gz(archive: &Path, dest: &Path) -> anyhow::Result<Vec<PathBuf>> {
    tracing::debug!(target: "LlamaDownloader", archive = %archive.display(), "解压 tar.gz");
    fs::create_dir_all(dest)?;

    let mut extracted_files = Vec::new();
    let tar_gz = fs::File::open(archive)?;
    let dec = flate2::read::GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(dec);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let out_path = dest.join(&path);

        // 记录包含 llama-server 的文件
        if path.file_name().map_or(false, |f| {
            let name = f.to_string_lossy();
            name == "llama-server" || name == "llama-server.exe"
        }) {
            extracted_files.push(out_path.clone());
        }

        entry.unpack(&out_path)?;
    }

    tracing::info!(target: "LlamaDownloader", count = extracted_files.len(), "解压完成，找到 llama-server 文件");
    Ok(extracted_files)
}

/// 解压 zip（Windows）
#[cfg(windows)]
#[allow(clippy::print_stderr)]
pub fn extract_zip(archive: &Path, dest: &Path) -> anyhow::Result<Vec<PathBuf>> {
    tracing::debug!(target: "LlamaDownloader", archive = %archive.display(), "解压 zip");
    fs::create_dir_all(dest)?;

    let mut extracted_files = Vec::new();

    // 先尝试 tar（某些 zip 实际是 tar 格式），失败则用 PowerShell Expand-Archive
    let result = extract_tar_gz(archive, dest);

    match result {
        Ok(files) if !files.is_empty() => {
            return Ok(files);
        }
        _ => {
            // 使用 PowerShell 解压
            let script = format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive.display(),
                dest.display()
            );
            let output = Command::new("powershell")
                .args(["-Command", &script])
                .output()
                .map_err(|e| anyhow::anyhow!("解压失败: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow::anyhow!("解压失败: {}", stderr));
            }
        }
    }

    // 递归查找 llama-server.exe
    find_llama_server_recursive(dest, &mut extracted_files)?;

    tracing::info!(target: "LlamaDownloader", count = extracted_files.len(), "解压完成，找到 llama-server 文件");
    Ok(extracted_files)
}

/// 递归查找 llama-server
#[cfg(windows)]
fn find_llama_server_recursive(dir: &Path, results: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                find_llama_server_recursive(&path, results)?;
            } else if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str == "llama-server.exe" || name_str == "llama-server" {
                    results.push(path);
                }
            }
        }
    }
    Ok(())
}

/// 校验 SHA256
pub fn verify_sha256(file: &Path, expected: &str) -> anyhow::Result<bool> {
    tracing::debug!(target: "LlamaDownloader", file = %file.display(), "校验 SHA256");
    let mut file = fs::File::open(file)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 65536];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 { break; }
        hasher.update(&buffer[..bytes_read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    let result = actual.eq_ignore_ascii_case(expected);
    tracing::info!(target: "LlamaDownloader", actual = %actual, expected = %expected, ok = result, "SHA256 校验完成");
    Ok(result)
}

/// 下载并安装 llama-server
#[allow(clippy::print_stderr)]
pub fn download_and_install(
    backend: GpuBackend,
    install_dir: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<DownloadResult> {
    let start = std::time::Instant::now();

    // 1. 获取最新版本
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "fetching_version".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: "获取最新版本...".to_string(),
        });
    }

    let release = fetch_llama_latest_release()?;
    let tag = &release.tag_name;
    tracing::debug!(target: "LlamaDownloader", tag = %tag, count = release.assets.len(), "最新版本");

    // 2. 构建资产名
    let asset_name = build_asset_name(tag, backend);
    tracing::debug!(target: "LlamaDownloader", asset = %asset_name, "查找匹配资产");

    // 3. 查找资产
    let asset = find_asset(&release, &asset_name)
        .ok_or_else(|| {
            // 列出所有可用资产帮助调试
            let available: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
            anyhow::anyhow!(
                "未找到资产 '{}'。\n可用资产: {:?}",
                asset_name, available
            )
        })?;

    tracing::info!(target: "LlamaDownloader", name = %asset.name, mb = asset.size as f64 / 1048576.0, "找到匹配资产");

    // 4. 下载
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "downloading".to_string(),
            progress: 0.1,
            downloaded: 0,
            total: asset.size,
            message: format!("下载 {} ({:.1} MB)...", asset.name, asset.size as f64 / 1048576.0),
        });
    }

    let ext = if asset.name.ends_with(".zip") { ".zip" } else { ".tar.gz" };
    let archive_path = install_dir.join(format!("llama{}{}", tag, ext));
    fs::create_dir_all(install_dir)?;

    let downloaded = download_file(&asset.browser_download_url, &archive_path, progress_callback)?;

    // 5. 解压
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "extracting".to_string(),
            progress: 0.9,
            downloaded,
            total: downloaded,
            message: "解压中...".to_string(),
        });
    }

    #[cfg(windows)]
    let extracted = extract_zip(&archive_path, install_dir)?;

    #[cfg(not(windows))]
    let extracted = extract_tar_gz(&archive_path, install_dir)?;

    // 6. 查找 llama-server
    let llama_server_path = extracted
        .into_iter()
        .find(|p| {
            p.file_name().map_or(false, |f| {
                let name = f.to_string_lossy();
                name == "llama-server" || name == "llama-server.exe"
            })
        })
        .ok_or_else(|| anyhow::anyhow!("解压后未找到 llama-server"))?;

    // 7. 设置可执行权限（Unix）
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&llama_server_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&llama_server_path, perms)?;
    }

    // 8. 删除归档文件
    let _ = fs::remove_file(&archive_path);

    let elapsed = start.elapsed().as_millis() as u64;
    let file_size = fs::metadata(&llama_server_path)?.len();

    tracing::info!(target: "LlamaDownloader", path = %llama_server_path.display(), bytes = file_size, elapsed_ms = elapsed, "安装完成");

    Ok(DownloadResult {
        success: true,
        path: llama_server_path.to_string_lossy().to_string(),
        file_size,
        sha256: String::new(),
        elapsed_ms: elapsed,
        error: None,
    })
}

/// 获取 llama.cpp 最新版本
#[allow(clippy::print_stderr)]
fn fetch_llama_latest_release() -> anyhow::Result<GitHubRelease> {
    let url = "https://api.github.com/repos/ggerganov/llama.cpp/releases/latest";
    tracing::debug!(target: "LlamaDownloader", url = url, "获取最新版本");

    let response = ureq::get(url)
        .set("User-Agent", "LlamaUI")
        .set("Accept", "application/vnd.github.v3+json")
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| anyhow::anyhow!("获取版本失败: {}", e))?;

    let release: GitHubRelease = response
        .into_json()
        .map_err(|e| anyhow::anyhow!("解析版本信息失败: {}", e))?;

    tracing::info!(target: "LlamaDownloader", tag = %release.tag_name, count = release.assets.len(), "获取到最新版本");

    Ok(release)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_backend_from_str() {
        assert_eq!(GpuBackend::from_str("cuda"), GpuBackend::Cuda12_4);
        assert_eq!(GpuBackend::from_str("cuda-12.4"), GpuBackend::Cuda12_4);
        assert_eq!(GpuBackend::from_str("cuda-13.3"), GpuBackend::Cuda13_3);
        assert_eq!(GpuBackend::from_str("cuda13"), GpuBackend::Cuda13_3);
        assert_eq!(GpuBackend::from_str("rocm"), GpuBackend::Rocm);
        assert_eq!(GpuBackend::from_str("vulkan"), GpuBackend::Vulkan);
        assert_eq!(GpuBackend::from_str("metal"), GpuBackend::Metal);
        assert_eq!(GpuBackend::from_str("cpu"), GpuBackend::Cpu);
        assert_eq!(GpuBackend::from_str("unknown"), GpuBackend::Cpu);
    }

    #[test]
    fn test_gpu_backend_as_str() {
        assert_eq!(GpuBackend::Cuda12_4.as_str(), "cuda-12.4");
        assert_eq!(GpuBackend::Cuda13_3.as_str(), "cuda-13.3");
        assert_eq!(GpuBackend::Rocm.as_str(), "rocm");
        assert_eq!(GpuBackend::Vulkan.as_str(), "vulkan");
        assert_eq!(GpuBackend::Metal.as_str(), "metal");
        assert_eq!(GpuBackend::Cpu.as_str(), "cpu");
    }

    #[test]
    fn test_build_asset_name_windows_cuda() {
        let name = build_asset_name("b10238", GpuBackend::Cuda12_4);
        assert_eq!(name, "llama-b10238-bin-win-cuda-12.4-x64.zip");
    }

    #[test]
    fn test_build_asset_name_windows_cpu() {
        let name = build_asset_name("b10238", GpuBackend::Cpu);
        // Windows: llama-b10238-bin-win-cpu-x64.zip
        assert!(name.contains("win-cpu"), "Windows CPU should include -cpu: {}", name);
    }

    #[test]
    fn test_build_asset_name_windows_vulkan() {
        let name = build_asset_name("b10238", GpuBackend::Vulkan);
        assert_eq!(name, "llama-b10238-bin-win-vulkan-x64.zip");
    }

    #[test]
    fn test_build_asset_name_linux_cpu() {
        // Linux 没有 cpu 后缀，使用 ubuntu
        let name = build_asset_name("b10238", GpuBackend::Cpu);
        // 在 Linux 上: llama-b10238-bin-ubuntu-x64.tar.gz
        // 在 Windows 上: llama-b10238-bin-win-x64.zip
        assert!(name.contains("llama-b10238-bin-"));
    }

    #[test]
    fn test_build_asset_name_macos_metal() {
        // macOS 只有一个包，不含 backend 标识
        // 在 macOS 上运行时验证
        if std::env::consts::OS == "macos" {
            let name = build_asset_name("b10238", GpuBackend::Metal);
            assert!(name.contains("macos"), "macOS should contain 'macos': {}", name);
            assert!(name.ends_with(".tar.gz"), "macOS should use tar.gz: {}", name);
        }
    }

    #[test]
    fn test_detect_gpu_backend() {
        let _backend = detect_gpu_backend();
    }
}
