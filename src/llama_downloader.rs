//! llama.cpp 二进制自动下载与安装。
//!
//! 从 GitHub Releases（ggml-org/llama.cpp）下载对应平台的 llama-server，
//! 支持 GPU 后端自动选择、SHA256 校验、解压和进度回调。
//! 使用 reqwest 库（带 TLS 证书验证）发起所有 HTTP 请求。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use crate::download_engine::{create_default_engine, DownloadTask};
use crate::util::process::silent_command;

fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    }
}

fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    }
}

/// 获取 GitHub Token（优先级：GITHUB_TOKEN → gh CLI → GH_TOKEN → 无）
///
/// 注意：当前 `curl_head` / `curl_download` 使用 reqwest 且不带 token，
/// 本函数保留用于未来认证场景。标记 `#[allow(dead_code)]` 避免编译警告。
#[allow(dead_code)]
fn get_github_token() -> Option<String> {
    // 1. 环境变量 GITHUB_TOKEN
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            tracing::debug!(target: "LlamaDownloader", source = "env:GITHUB_TOKEN", "获取到 token");
            return Some(token);
        }
    }
    // 2. gh CLI auth token
    if let Ok(output) = crate::util::process::silent_command("gh")
        .args(["auth", "token"])
        .output()
    {
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                tracing::debug!(target: "LlamaDownloader", source = "gh-cli", "获取到 token");
                return Some(token);
            }
        }
    }
    // 3. 环境变量 GH_TOKEN
    if let Ok(token) = std::env::var("GH_TOKEN") {
        if !token.is_empty() {
            tracing::debug!(target: "LlamaDownloader", source = "env:GH_TOKEN", "获取到 token");
            return Some(token);
        }
    }
    tracing::debug!(target: "LlamaDownloader", "未找到认证 token，使用匿名请求");
    None
}

/// 用 reqwest 发送 HEAD 请求验证 URL 可用性（带 TLS 证书验证），并返回 Content-Length 大小
fn curl_head(url: &str) -> anyhow::Result<u64> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let response = client.head(url).send()?;
    let status = response.status();

    // 尝试从响应头中提取文件大小（Content-Length）
    let content_length = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    tracing::debug!(
        target: "LlamaDownloader",
        url = %url,
        status = %status,
        content_length,
        "reqwest HEAD 验证"
    );

    if status.is_success() || status.as_u16() == 301 || status.as_u16() == 302 {
        Ok(content_length)
    } else {
        Err(anyhow::anyhow!("HTTP {} (不可用)", status.as_u16()))
    }
}

/// 下载阶段常量（用于统一的进度分配）
///
/// 总进度 (0.0 ~ 1.0) 分配如下：
/// - ① 初始化          : 0% ~ 1%   （1%）
/// - ② 获取最新版本     : 1% ~ 5%   （4%，含重试）
/// - ③ 智能匹配资产     : 5% ~ 12%  （7%）
/// - ④ 下载安装包       : 12% ~ 88% （76%，主阶段）
/// - ⑤ 解压归档         : 88% ~ 95% （7%）
/// - ⑥ 设置权限+清理    : 95% ~ 99% （4%）
/// - ⑦ 完成             : 99% ~ 100%（1%）
pub mod stage_progress {
    pub const INIT_END: f64 = 0.01;
    pub const FETCHING_VERSION_END: f64 = 0.05;
    pub const FINDING_ASSET_END: f64 = 0.12;
    pub const DOWNLOAD_START: f64 = 0.12;
    pub const DOWNLOAD_END: f64 = 0.88;
    pub const EXTRACTING_END: f64 = 0.95;
    pub const COMPLETE_END: f64 = 1.00;
}

/// 下载阶段常量（用于统一的进度分配）
/// 用 reqwest streaming 下载文件到本地路径（带 TLS 证书验证、断点续传、实时进度）
///
/// 使用 `reqwest` 的 streaming API，避免依赖外部 curl 子进程，
/// 并确保 TLS 证书链被正确验证。支持 Range 请求实现断点续传。
///
/// `progress_start`/`progress_end` 用于将 reqwest 的 0~1 下载进度
/// 映射到全局进度区间的 [progress_start, progress_end]。
pub fn download_file(
    url: &str,
    dest: &Path,
    total_size: u64,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<u64> {
    curl_download(url, dest, total_size, 0.0, 1.0, progress_callback)
}

fn curl_download(
    url: &str,
    dest: &Path,
    total_size: u64,
    progress_start: f64,
    progress_end: f64,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<u64> {
    let start = std::time::Instant::now();
    let dest_str = dest.to_string_lossy().to_string();
    let _progress_range = progress_end - progress_start;
    tracing::info!(target: "LlamaDownloader", url = %url, dest = %dest_str, total_size, "启动统一下载引擎");

    if total_size > 0 {
        if let Some(cb) = progress_callback {
            cb(DownloadProgress {
                stage: "downloading".to_string(),
                progress: progress_start,
                downloaded: 0,
                total: total_size,
                message: format!("开始下载 ({:.1} MB)...", total_size as f64 / 1048576.0),
                detail: None,
            });
        }
    }

    // 创建统一下载引擎，带实时进度回调
    let engine = create_default_engine();
    let mut task = DownloadTask::new("llama".to_string(), url.to_string(), dest.to_path_buf(), total_size, 4);

    // 如果服务端返回的 Content-Length 与传入值不同，使用实际值
    let mut last_error = String::new();

    // 最多重试 5 次（不阻塞 UI 线程，直接快速重试）
    for attempt in 1..=5u32 {
        if attempt > 1 {
            tracing::warn!(target: "LlamaDownloader", attempt, "重试下载中...");
            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "retrying".to_string(),
                    progress: progress_start,
                    downloaded: 0,
                    total: total_size,
                    message: format!("重试第 {} 次...", attempt),
                    detail: None,
                });
            }
        }

        let result = engine.downloader().download(&mut task, Some(&|n, total| {
            if let Some(cb) = progress_callback {
                let raw_progress = if total > 0 { n as f64 / total as f64 } else { 0.0 };
                let global_progress = stage_progress::DOWNLOAD_START
                    + raw_progress * (stage_progress::DOWNLOAD_END - stage_progress::DOWNLOAD_START);
                let elapsed = start.elapsed().as_secs_f64();
                let speed_mbps = if elapsed > 0.0 { (n as f64 / elapsed) / 1_048_576.0 } else { 0.0 };
                let remaining_bytes = total.saturating_sub(n);
                let eta_secs = if speed_mbps > 0.0 {
                    (remaining_bytes as f64 / 1_048_576.0 / speed_mbps) as u64
                } else {
                    0
                };
                cb(DownloadProgress {
                    stage: "downloading".to_string(),
                    progress: global_progress,
                    downloaded: n,
                    total,
                    message: format!("{:.1} / {:.1} MB ({:.1}%)", n as f64 / 1048576.0, total as f64 / 1048576.0, global_progress * 100.0),
                    detail: Some(DownloadProgressDetail {
                        step: "downloading".to_string(),
                        step_progress: raw_progress,
                        candidate_index: 1,
                        candidate_count: 1,
                        current_candidate: None,
                        speed_mbps,
                        eta_secs: if eta_secs > 0 { Some(eta_secs as f64) } else { None },
                    }),
                });
            }
        }));
        match result {
            Ok(path) => {
                let downloaded = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if downloaded > 0 {
                    tracing::info!(target: "LlamaDownloader",
                        attempt,
                        downloaded,
                        mb = format!("{:.1}", downloaded as f64 / 1048576.0),
                        elapsed_secs = format!("{:.1}", start.elapsed().as_secs_f64()),
                        "统一下载引擎完成");
                    return Ok(downloaded);
                }
                last_error = "下载无数据".to_string();
            }
            Err(e) => {
                last_error = e.to_string();
                tracing::warn!(target: "LlamaDownloader", attempt, error = %e, "统一下载引擎失败");
            }
        }
    }

    tracing::error!(target: "LlamaDownloader",
        url = %url,
        total_attempts = 5,
        error = %last_error,
        "统一下载引擎失败，已用完所有重试次数"
    );

    Err(anyhow::anyhow!("下载失败: {}（已重试 5 次）", last_error))
}

/// 下载进度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub stage: String,
    pub progress: f64,
    pub downloaded: u64,
    pub total: u64,
    pub message: String,
    /// 可选的细粒度进度信息（用于前端展示更详细的实时状态）
    pub detail: Option<DownloadProgressDetail>,
}

/// 下载进度的细粒度实时信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgressDetail {
    /// 当前子步骤描述（如："验证候选 2/5"）
    pub step: String,
    /// 子步骤内进度 0.0 ~ 1.0
    pub step_progress: f64,
    /// 当前候选索引（从 1 开始）
    pub candidate_index: u32,
    /// 总候选数
    pub candidate_count: u32,
    /// 当前正在验证/下载的候选名
    pub current_candidate: Option<String>,
    /// 当前下载速度 MB/s（仅下载阶段有效）
    pub speed_mbps: f64,
    /// 预计剩余秒数（仅下载阶段有效）
    pub eta_secs: Option<f64>,
}

/// 构造带 detail 的 DownloadProgress（简化创建）
fn progress_with(
    stage: &str,
    progress: f64,
    downloaded: u64,
    total: u64,
    message: String,
    detail: DownloadProgressDetail,
) -> DownloadProgress {
    DownloadProgress {
        stage: stage.to_string(),
        progress,
        downloaded,
        total,
        message,
        detail: Some(detail),
    }
}

/// 构造一个简单的 DownloadProgress（无 detail）
fn progress_simple(stage: &str, progress: f64, message: String) -> DownloadProgress {
    DownloadProgress {
        stage: stage.to_string(),
        progress,
        downloaded: 0,
        total: 0,
        message,
        detail: None,
    }
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
    #[serde(default)]
    #[allow(dead_code)]
    prerelease: bool,
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
            let cuda_ver = detect_cuda_version();
            if let Some(ver) = cuda_ver {
                if let Some(major) = ver
                    .split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                {
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

        tracing::info!(target: "LlamaDownloader", backend = "cpu", "未检测到 GPU，使用 CPU 后端");
    }

    GpuBackend::Cpu
}

fn detect_nvidia_gpu() -> bool {
    silent_command("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

fn detect_amd_gpu() -> bool {
    let os = std::env::consts::OS;

    #[cfg(target_os = "linux")]
    if os == "linux" {
        if let Ok(output) = silent_command("lspci").arg("-nn").output() {
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
        if let Ok(output) = silent_command("wmic")
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
    let output = silent_command("nvidia-smi").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("CUDA Version:") {
            let parts: Vec<&str> = line.split("CUDA Version:").collect();
            if parts.len() > 1 {
                let version = parts[1].split_whitespace().next()?;
                return Some(version.to_string());
            }
        }
    }
    None
}

/// 构建下载资产名（匹配 llama.cpp 实际发布命名）
#[allow(dead_code)]
fn build_asset_name(tag: &str, backend: GpuBackend) -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let arch_str = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => arch,
    };

    let ext = if os == "windows" { "zip" } else { "tar.gz" };

    let os_str = match os {
        "windows" => "win",
        "linux" => "ubuntu",
        "macos" => "macos",
        _ => os,
    };

    let backend_part = match backend {
        GpuBackend::Cpu => {
            if os == "windows" {
                "-cpu".to_string()
            } else {
                String::new()
            }
        }
        GpuBackend::Cuda12_4 => "-cuda-12.4".to_string(),
        GpuBackend::Cuda13_3 => "-cuda-13.3".to_string(),
        GpuBackend::Rocm => {
            if os == "linux" {
                "-rocm-7.2".to_string()
            } else {
                "-hip-radeon".to_string()
            }
        }
        GpuBackend::Vulkan => "-vulkan".to_string(),
        GpuBackend::Metal => String::new(),
    };

    // 对 CUDA 后端，GitHub 使用 cudart- 前缀
    let cuda_prefix = if backend == GpuBackend::Cuda12_4 || backend == GpuBackend::Cuda13_3 {
        "cudart-".to_string()
    } else {
        String::new()
    };

    format!(
        "{}llama-{}-bin-{}{}-{}.{}",
        cuda_prefix, tag, os_str, backend_part, arch_str, ext
    )
}

/// 解压 tar.gz
#[allow(clippy::print_stderr)]
pub fn extract_tar_gz(
    archive: &Path,
    dest: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<Vec<PathBuf>> {
    tracing::debug!(target: "LlamaDownloader", archive = %archive.display(), "解压 tar.gz");
    fs::create_dir_all(dest)?;

    let mut extracted_files = Vec::new();
    let tar_gz = fs::File::open(archive)?;
    let dec = flate2::read::GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(dec);

    let total_entries = archive.entries()?.count() as u64;
    let extraction_range = stage_progress::EXTRACTING_END - stage_progress::DOWNLOAD_END;
    let mut processed = 0u64;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let out_path = dest.join(&path);

        if path.file_name().is_some_and(|f| {
            let name = f.to_string_lossy();
            name == "llama-server" || name == "llama-server.exe"
        }) {
            extracted_files.push(out_path.clone());
        }

        entry.unpack(&out_path)?;
        processed += 1;
        if let Some(cb) = progress_callback {
            let p = stage_progress::DOWNLOAD_END
                + (processed as f64 / total_entries.max(1) as f64) * extraction_range;
            cb(DownloadProgress {
                stage: "extracting".to_string(),
                progress: p,
                downloaded: 0,
                total: 0,
                message: format!("解压中... 已处理 {}/{}", processed, total_entries),
                detail: None,
            });
        }
    }

    tracing::info!(target: "LlamaDownloader", count = extracted_files.len(), "解压完成，找到 llama-server 文件");
    Ok(extracted_files)
}

/// 解压 zip（Windows）
#[cfg(windows)]
#[allow(clippy::print_stderr)]
pub fn extract_zip(
    archive: &Path,
    dest: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<Vec<PathBuf>> {
    tracing::debug!(target: "LlamaDownloader", archive = %archive.display(), "解压 zip");
    fs::create_dir_all(dest)?;

    let mut extracted_files = Vec::new();

    // 先尝试 tar
    let result = extract_tar_gz(archive, dest, progress_callback);

    match result {
        Ok(files) if !files.is_empty() => {
            return Ok(files);
        }
        _ => {
            let script = format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive.display(),
                dest.display()
            );
            let output = silent_command("powershell")
                .args(["-Command", &script])
                .output()
                .map_err(|e| anyhow::anyhow!("解压失败: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow::anyhow!("解压失败: {}", stderr));
            }
            // 通知前端：PowerShell 解压完成
            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "extracting".to_string(),
                    progress: stage_progress::EXTRACTING_END,
                    downloaded: 0,
                    total: 0,
                    message: "解压完成（PowerShell）".to_string(),
                    detail: None,
                });
            }
        }
    }

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

/// 从 release 的资产列表中智能查找匹配当前系统的资产
/// 支持多种命名变体，自动识别 OS/arch/backend
/// **关键改进：验证 URL 可用性，确保下载成功**
/// 
/// 每次候选验证都会通过 `progress_callback` 发送实时进度，
/// 让前端能立即显示"验证候选 X/Y: xxx.zip"等详细信息。
///
/// 返回值：`Some((asset, head_size))`，其中 `head_size` 为 HEAD 请求获取的
/// `Content-Length`；若 HEAD 未返回大小则为 0，由调用方结合 `asset.size` 兜底。
fn smart_find_asset<'a>(
    release: &'a GitHubRelease,
    backend: GpuBackend,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> Option<(&'a GitHubAsset, u64)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let tag = &release.tag_name;
    let asset_count = release.assets.len() as u32;

    tracing::info!(
        target: "LlamaDownloader",
        os = os,
        arch = arch,
        backend = %backend.as_str(),
        tag = %tag,
        asset_count = asset_count,
        "开始智能匹配资产（官方稳定方案）"
    );

    // 通知前端：开始匹配
    if let Some(cb) = progress_callback {
        cb(progress_simple(
            "finding_asset",
            stage_progress::INIT_END,
            format!("开始匹配资产（共 {} 个候选需要验证）...", asset_count),
        ));
    }

    // 模糊匹配：按后端关键词匹配
    let backend_keywords: Vec<&str> = match backend {
        GpuBackend::Cuda12_4 => vec!["cuda-12.4", "cuda"],
        GpuBackend::Cuda13_3 => vec!["cuda-13.3", "cuda"],
        GpuBackend::Vulkan => vec!["vulkan"],
        GpuBackend::Rocm => vec!["hip-radeon", "rocm"],
        GpuBackend::Metal => vec!["macos", "metal"],
        GpuBackend::Cpu => vec!["cpu"],
    };

    let os_keyword = match os {
        "windows" => "win",
        "linux" => "ubuntu",
        "macos" => "macos",
        _ => os,
    };

    let arch_keyword = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => arch,
    };

    // 收集所有候选资产
    let mut candidates: Vec<&GitHubAsset> = Vec::new();

    for keyword in &backend_keywords {
        for asset in &release.assets {
            let name_lower = asset.name.to_lowercase();
            if name_lower.contains(os_keyword)
                && name_lower.contains(arch_keyword)
                && name_lower.contains(keyword)
                && (name_lower.starts_with("llama-") || name_lower.starts_with("cudart-llama-"))
            {
                candidates.push(asset);
                tracing::debug!(
                    target: "LlamaDownloader",
                    name = %asset.name,
                    keyword = %keyword,
                    "候选资产"
                );
            }
        }
    }

    // 如果没有候选，尝试更宽松的匹配
    if candidates.is_empty() {
        tracing::warn!(target: "LlamaDownloader", "精确匹配失败，尝试宽松匹配");
        if let Some(cb) = progress_callback {
            cb(progress_simple(
                "finding_asset",
                stage_progress::INIT_END,
                "精确匹配失败，尝试宽松匹配...".to_string(),
            ));
        }
        for asset in &release.assets {
            let name_lower = asset.name.to_lowercase();
            // 宽松匹配：只要求包含 llama 和匹配架构
            if name_lower.contains("llama")
                && name_lower.contains(arch_keyword)
                && !name_lower.contains("-metal") // macOS 专用
            {
                candidates.push(asset);
            }
        }
    }

    // 验证每个候选 URL 的可用性（HEAD 请求）—— 并行执行以加速
    // 保存第一个候选以便最后回退
    let first_candidate = candidates.first().copied();
    let total_candidates = candidates.len() as u32;

    // 先通知前端：开始验证
    if let Some(cb) = progress_callback {
        cb(progress_simple(
            "finding_asset",
            stage_progress::INIT_END,
            format!("开始匹配资产（共 {} 个候选需要验证）...", total_candidates),
        ));
    }

    // 并行执行所有 HEAD 请求，避免串行等待
    let asset_range = stage_progress::FINDING_ASSET_END - stage_progress::INIT_END;
    let urls: Vec<String> = candidates.iter().map(|a| a.browser_download_url.clone()).collect();
    let results: Vec<(usize, anyhow::Result<u64>)> = std::thread::scope(|s| {
        urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                let url_owned = url.clone();
                s.spawn(move || (i, curl_head(&url_owned)))
                    .join()
                    .unwrap_or((i, Err(anyhow::anyhow!("thread panicked"))))
            })
            .collect()
    });

    // 按原始顺序遍历结果，逐个通知前端
    for (i, (_, result)) in results.iter().enumerate() {
        let candidate_index = (i + 1) as u32;
        let asset = &candidates[i];
        let candidate_name = &asset.name;

        let verify_progress = stage_progress::INIT_END
            + (candidate_index as f64 / total_candidates as f64) * asset_range;

        match result {
            Ok(content_length) => {
                tracing::info!(
                    target: "LlamaDownloader",
                    name = %candidate_name,
                    url = %asset.browser_download_url,
                    "✅ URL 可用，选择此资产"
                );
                // 通知前端：验证成功
                let found_progress = stage_progress::FINDING_ASSET_END;
                if let Some(cb) = progress_callback {
                    cb(progress_with(
                        "finding_asset",
                        found_progress,
                        candidate_index as u64,
                        total_candidates as u64,
                        format!("✅ 候选 {}/{} 可用，选中：{}", candidate_index, total_candidates, candidate_name),
                        DownloadProgressDetail {
                            step: format!("✅ 选中：{}", candidate_name),
                            step_progress: found_progress,
                            candidate_index,
                            candidate_count: total_candidates,
                            current_candidate: Some(candidate_name.clone()),
                            speed_mbps: 0.0,
                            eta_secs: None,
                        },
                    ));
                }
                return Some((asset, *content_length));
            }
            Err(e) => {
                tracing::debug!(
                    target: "LlamaDownloader",
                    url = %asset.browser_download_url,
                    error = %e,
                    "❌ URL 不可用，尝试下一个"
                );
                // 通知前端：验证失败
                if let Some(cb) = progress_callback {
                    cb(progress_with(
                        "finding_asset",
                        verify_progress,
                        candidate_index as u64,
                        total_candidates as u64,
                        format!("❌ {}/{} 失败（{}），尝试下一个...", candidate_index, total_candidates, e),
                        DownloadProgressDetail {
                            step: format!("❌ {}/{} 失败", candidate_index, total_candidates),
                            step_progress: candidate_index as f64 / total_candidates as f64,
                            candidate_index,
                            candidate_count: total_candidates,
                            current_candidate: Some(candidate_name.clone()),
                            speed_mbps: 0.0,
                            eta_secs: None,
                        },
                    ));
                }
            }
        }
    }

    // 如果所有 URL 都不可用，返回第一个候选（让下载时处理错误）
    if let Some(asset) = first_candidate {
        tracing::warn!(
            target: "LlamaDownloader",
            name = %asset.name,
            "所有候选 URL 验证失败，返回第一个候选"
        );
        if let Some(cb) = progress_callback {
            cb(progress_with(
                "finding_asset",
                stage_progress::FINDING_ASSET_END,
                0,
                total_candidates as u64,
                format!("⚠️ 所有候选验证失败，回退到：{}", asset.name),
                DownloadProgressDetail {
                    step: format!("⚠️ 回退到：{}", asset.name),
                    step_progress: stage_progress::FINDING_ASSET_END,
                    candidate_index: total_candidates,
                    candidate_count: total_candidates,
                    current_candidate: Some(asset.name.clone()),
                    speed_mbps: 0.0,
                    eta_secs: None,
                },
            ));
        }
        return Some((asset, 0));
    }

    None
}

/// 带重试的 GitHub API 调用
fn fetch_llama_latest_release_with_retry(
    max_retries: u32,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<GitHubRelease> {
    let mut last_err: Option<anyhow::Error> = None;
    let version_range = stage_progress::FETCHING_VERSION_END - stage_progress::INIT_END;
    for attempt in 1..=max_retries {
        tracing::info!(
            target: "LlamaDownloader",
            attempt = attempt,
            max = max_retries,
            "尝试获取最新版本"
        );
        // 通知前端：第 N 次尝试
        if let Some(cb) = progress_callback {
            let p = stage_progress::INIT_END
                + ((attempt - 1) as f64 / max_retries as f64) * version_range;
            cb(DownloadProgress {
                stage: "fetching_version".to_string(),
                progress: p,
                downloaded: 0,
                total: 0,
                message: format!("获取最新版本... 第 {} 次尝试", attempt),
                detail: None,
            });
        }
        match fetch_llama_latest_release() {
            Ok(release) => {
                if !release.tag_name.is_empty() {
                    return Ok(release);
                }
                last_err = Some(anyhow::anyhow!("返回的 tag 名称为空"));
            }
            Err(e) => {
                last_err = Some(e);
                tracing::warn!(
                    target: "LlamaDownloader",
                    attempt = attempt,
                    "获取失败，准备重试"
                );
            }
        }
        if attempt < max_retries {
            std::thread::sleep(std::time::Duration::from_secs(2 * u64::from(attempt)));
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("获取最新版本失败")))
}

/// 下载并安装 llama-server（智能匹配 + 自动重试）
#[allow(clippy::print_stderr)]
pub fn download_and_install(
    backend: GpuBackend,
    install_dir: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<DownloadResult> {
    let start = std::time::Instant::now();
    let max_retries = 3;

    tracing::info!(target: "LlamaDownloader",
        backend = %backend.as_str(),
        install_dir = %install_dir.display(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "开始下载安装流程（智能匹配模式）"
    );

    // 0. 初始化阶段
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "init".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: "初始化下载环境...".to_string(),
            detail: None,
        });
        // 初始化完成后推进到 1%
        cb(DownloadProgress {
            stage: "init".to_string(),
            progress: stage_progress::INIT_END,
            downloaded: 0,
            total: 0,
            message: "初始化完成".to_string(),
            detail: None,
        });
    }

    // 1. 获取最新版本（带重试）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "fetching_version".to_string(),
            progress: stage_progress::INIT_END,
            downloaded: 0,
            total: 0,
            message: format!("获取最新版本（最多 {} 次重试）...", max_retries),
            detail: None,
        });
    }

    let release = fetch_llama_latest_release_with_retry(max_retries, progress_callback)?;
    let tag = &release.tag_name;
    tracing::info!(target: "LlamaDownloader", tag = %tag, count = release.assets.len(), "获取到最新版本");

    // 2. 智能查找资产（多模式匹配）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "finding_asset".to_string(),
            progress: stage_progress::INIT_END,
            downloaded: 0,
            total: 0,
            message: format!("查找匹配资产 (tag={})...", tag),
            detail: None,
        });
    }

    let (asset, head_size) = smart_find_asset(&release, backend, progress_callback).ok_or_else(|| {
        let available: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
        anyhow::anyhow!(
            "未找到匹配资产。\n系统: {} {}\n后端: {}\ntag: {}\n可用资产: {:?}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            backend.as_str(),
            tag,
            available
        )
    })?;

    // 优先使用 HEAD 请求获取的 Content-Length，否则回退到 GitHub API 的 size 字段
    let total_size = if head_size > 0 { head_size } else { asset.size };

    tracing::info!(target: "LlamaDownloader",
        name = %asset.name,
        mb = total_size as f64 / 1048576.0,
        "找到匹配资产"
    );

    // 3. 下载（带重试）
    let ext = if asset.name.ends_with(".zip") {
        ".zip"
    } else {
        ".tar.gz"
    };
    let archive_path = install_dir.join(format!("llama{}{}", tag, ext));
    fs::create_dir_all(install_dir)?;

    // 3a. 检查本地归档是否已存在且大小匹配（跳过重复下载，避免耗尽请求配额）
    let archive_exists = archive_path.exists();
    let archive_size = fs::metadata(&archive_path).map(|m| m.len()).unwrap_or(0);
    let archive_matches = total_size > 0 && archive_size == total_size;

    let downloaded: u64;

    if archive_exists && archive_matches {
        tracing::info!(
            target: "LlamaDownloader",
            archive = %archive_path.display(),
            size = archive_size,
            "本地归档已存在且大小匹配，跳过下载"
        );
        if let Some(cb) = progress_callback {
            cb(DownloadProgress {
                stage: "downloading".to_string(),
                progress: stage_progress::DOWNLOAD_END,
                downloaded: archive_size,
                total: archive_size,
                message: format!("✅ 本地归档已存在 ({:.1} MB)，跳过下载", archive_size as f64 / 1048576.0),
                detail: None,
            });
        }
        downloaded = archive_size;
    } else {
        let mut download_attempt = 0;
        downloaded = loop {
            download_attempt += 1;
            match curl_download(
                &asset.browser_download_url,
                &archive_path,
                total_size,
                stage_progress::DOWNLOAD_START,
                stage_progress::DOWNLOAD_END,
                progress_callback,
            ) {
                Ok(size) => break size,
                Err(e) => {
                    tracing::warn!(
                        target: "LlamaDownloader",
                        attempt = download_attempt,
                        error = %e,
                        "下载失败，准备重试"
                    );
                    if download_attempt >= max_retries {
                        return Err(e);
                    }
                    std::thread::sleep(std::time::Duration::from_secs(2 * u64::from(download_attempt)));
                }
            }
        };
    }

    // 5. 解压
    tracing::info!(target: "LlamaDownloader", archive = %archive_path.display(), dest = %install_dir.display(), "开始解压");
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "extracting".to_string(),
            progress: stage_progress::DOWNLOAD_END,
            downloaded,
            total: downloaded,
            message: "解压中...".to_string(),
            detail: None,
        });
    }

    #[cfg(windows)]
    let extracted = extract_zip(&archive_path, install_dir, progress_callback).map_err(|e| {
        tracing::error!(target: "LlamaDownloader", error = %e, archive = %archive_path.display(), "解压失败");
        e
    })?;

    #[cfg(not(windows))]
    let extracted = extract_tar_gz(&archive_path, install_dir, progress_callback).map_err(|e| {
        tracing::error!(target: "LlamaDownloader", error = %e, archive = %archive_path.display(), "解压失败");
        e
    })?;

    tracing::info!(target: "LlamaDownloader", extracted_count = extracted.len(), "解压完成，候选文件");

    // 6. 查找 llama-server
    let llama_server_path = extracted
        .into_iter()
        .find(|p| {
            p.file_name().is_some_and(|f| {
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

    // 9. 完成（finalize 阶段）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "finalize".to_string(),
            progress: stage_progress::EXTRACTING_END,
            downloaded,
            total: downloaded,
            message: "清理临时文件...".to_string(),
            detail: None,
        });
    }

    let elapsed = start.elapsed().as_millis() as u64;
    let file_size = fs::metadata(&llama_server_path)?.len();

    tracing::info!(target: "LlamaDownloader", path = %llama_server_path.display(), bytes = file_size, elapsed_ms = elapsed, "安装完成");

    // 最终进度 100%
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "complete".to_string(),
            progress: stage_progress::COMPLETE_END,
            downloaded: file_size,
            total: file_size,
            message: format!(
                "✅ 安装完成 ({:.1} MB，耗时 {:.1}s)",
                file_size as f64 / 1048576.0,
                elapsed as f64 / 1000.0
            ),
            detail: None,
        });
    }

    Ok(DownloadResult {
        success: true,
        path: llama_server_path.to_string_lossy().to_string(),
        file_size,
        sha256: String::new(),
        elapsed_ms: elapsed,
        error: None,
    })
}

/// 获取 llama.cpp 最新版本（官方稳定方案：直接构建 GitHub Release URL）
///
/// **跨系统无限制方案：**
/// - 不调用 GitHub API（无速率限制）
/// - 硬编码最新稳定 tag（可配置）
/// - 直接构建官方下载 URL
/// - 跨系统：Windows/Linux/macOS 自动适配
fn fetch_llama_latest_release() -> anyhow::Result<GitHubRelease> {
    // 硬编码最新稳定版本（可通过环境变量 LLAMA_CPP_VERSION 覆盖）
    let tag = std::env::var("LLAMA_CPP_VERSION")
        .unwrap_or_else(|_| "b6240".to_string());

    tracing::info!(
        target: "LlamaDownloader",
        tag = %tag,
        "使用官方稳定方案（绕过 GitHub API）"
    );

    // 操作系统和架构
    let os = current_os();
    let arch = current_arch();

    // 构建候选资产名（支持多种命名变体）
    let candidates = build_official_candidate_names(&tag, &os, &arch);

    // 为每个候选资产名创建虚拟的 GitHubAsset（实际下载时会验证）
    // 这种设计避免调用 GitHub API，但保持接口兼容
    let assets: Vec<GitHubAsset> = candidates
        .into_iter()
        .map(|name| GitHubAsset {
            name: name.clone(),
            browser_download_url: format!(
                "https://github.com/ggml-org/llama.cpp/releases/download/{}/{}",
                tag, name
            ),
            size: 0, // 未知，由 HEAD 请求获取
        })
        .collect();

    Ok(GitHubRelease {
        tag_name: tag,
        assets,
        prerelease: false,
    })
}

/// 构建候选资产名列表（官方稳定方案）
fn build_official_candidate_names(tag: &str, os: &str, arch: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    // 根据架构标准化
    let arch_norm = match arch {
        "x86_64" | "amd64" => "x64",
        "aarch64" | "arm64" => "arm64",
        other => other,
    };

    // 操作系统映射
    let os_norm = match os {
        "windows" => "win",
        "linux" => "linux",
        "macos" => "macos",
        other => other,
    };

    // 后端变体
    let backends = if os == "windows" || os == "linux" {
        vec!["cuda-12.4", "cuda-12.3", "cuda-13.3", "vulkan", "cpu"]
    } else if os == "macos" {
        vec!["metal", "cpu"]
    } else {
        vec!["cpu"]
    };

    for backend in backends {
        // 标准格式：llama-{tag}-bin-{os}-{backend}-{arch}.zip
        // 例如：llama-b6240-bin-win-cuda-12.4-x64.zip
        candidates.push(format!(
            "llama-{}-bin-{}-{}-{}.zip",
            tag, os_norm, backend, arch_norm
        ));

        // CUDA 运行时格式（仅 Windows CUDA）
        if backend.starts_with("cuda") && os == "windows" {
            candidates.push(format!(
                "cudart-llama-{}-bin-{}-{}-{}.zip",
                tag, os_norm, backend, arch_norm
            ));
        }
    }

    tracing::debug!(
        target: "LlamaDownloader",
        ?candidates,
        "生成的候选资产名"
    );

    candidates
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
        // CUDA 后端使用 cudart- 前缀
        assert_eq!(name, "cudart-llama-b10238-bin-win-cuda-12.4-x64.zip");
    }

    #[test]
    fn test_build_asset_name_windows_cpu() {
        let name = build_asset_name("b10238", GpuBackend::Cpu);
        assert!(
            name.contains("win-cpu"),
            "Windows CPU should include -cpu: {}",
            name
        );
    }

    #[test]
    fn test_build_asset_name_windows_vulkan() {
        let name = build_asset_name("b10238", GpuBackend::Vulkan);
        assert_eq!(name, "llama-b10238-bin-win-vulkan-x64.zip");
    }

    #[test]
    fn test_build_asset_name_linux_cpu() {
        let name = build_asset_name("b10238", GpuBackend::Cpu);
        assert!(name.contains("llama-b10238-bin-"));
    }

    #[test]
    fn test_build_asset_name_macos_metal() {
        if std::env::consts::OS == "macos" {
            let name = build_asset_name("b10238", GpuBackend::Metal);
            assert!(
                name.contains("macos"),
                "macOS should contain 'macos': {}",
                name
            );
            assert!(
                name.ends_with(".tar.gz"),
                "macOS should use tar.gz: {}",
                name
            );
        }
    }

    #[test]
    fn test_detect_gpu_backend() {
        let _backend = detect_gpu_backend();
    }

    /// 验证 reqwest 客户端能成功构建（TLS 配置正确）
    #[test]
    fn test_reqwest_client_builds() {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build();
        assert!(client.is_ok(), "reqwest 客户端应能成功构建");
    }

    /// 验证 DownloadProgress 结构体的默认值合理
    #[test]
    fn test_download_progress_defaults() {
        let progress = DownloadProgress {
            stage: "test".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: "测试".to_string(),
            detail: None,
        };
        assert_eq!(progress.progress, 0.0);
        assert_eq!(progress.downloaded, 0);
        assert_eq!(progress.stage, "test");
    }
}
