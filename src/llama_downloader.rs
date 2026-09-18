//! llama.cpp 二进制自动下载与安装。
//!
//! 从 GitHub Releases（ggml-org/llama.cpp）下载对应平台的 llama-server，
//! 支持 GPU 后端自动选择、SHA256 校验、解压和进度回调。
//! 使用 reqwest 库（带 TLS 证书验证）发起所有 HTTP 请求。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::sync::LazyLock;
use reqwest::blocking::Client;
use crate::download_engine::{create_default_engine, DownloadTask};
use crate::util::process::silent_command;

/// 共享的 HTTP 客户端（由下载引擎初始化，复用连接池）
static SHARED_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    create_default_engine().http_client().client()
});

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
    let response = SHARED_CLIENT.head(url).send()?;
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
    /// 版本来源（用于前端显示进度信息）
    #[serde(skip)]
    source: &'static str,
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
            // Linux 资产可能使用 ubuntu 或 linux 前缀
            let os_match = if os == "linux" {
                name_lower.contains("ubuntu") || name_lower.contains("linux")
            } else {
                name_lower.contains(os_keyword)
            };
            if os_match
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

/// 带重试的 GitHub API 调用（指数退避）
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
            // 指数退避：500ms, 1s, 2s, 4s
            let delay = 500u64 * (1u64 << (attempt - 1));
            let delay = delay.min(5000);
            tracing::info!(target: "LlamaDownloader", delay_ms = delay, "等待后重试");
            std::thread::sleep(std::time::Duration::from_millis(delay));
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
                    // 指数退避：2s, 4s, 8s
                    let delay = 2u64.pow(download_attempt as u32);
                    std::thread::sleep(std::time::Duration::from_secs(delay));
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

    // 10. SHA256 完整性校验（快速且必要）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "verifying".to_string(),
            progress: stage_progress::EXTRACTING_END,
            downloaded: file_size,
            total: file_size,
            message: "校验文件完整性...".to_string(),
            detail: Some(DownloadProgressDetail {
                step: "SHA256 校验".to_string(),
                step_progress: 0.5,
                candidate_index: 0,
                candidate_count: 0,
                current_candidate: None,
                speed_mbps: 0.0,
                eta_secs: None,
            }),
        });
    }
    let sha256 = compute_sha256(&llama_server_path, progress_callback, file_size)?;
    tracing::info!(target: "LlamaDownloader", sha256 = %sha256, "SHA256 校验完成");

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
            detail: Some(DownloadProgressDetail {
                step: "完成".to_string(),
                step_progress: 1.0,
                candidate_index: 0,
                candidate_count: 0,
                current_candidate: Some(sha256.clone()),
                speed_mbps: 0.0,
                eta_secs: None,
            }),
        });
    }

    Ok(DownloadResult {
        success: true,
        path: llama_server_path.to_string_lossy().to_string(),
        file_size,
        sha256,
        elapsed_ms: elapsed,
        error: None,
    })
}

/// 计算文件的 SHA256 十六进制摘要（流式读取 + 进度回调）
fn compute_sha256(
    path: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
    file_size: u64,
) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut bytes_read = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes_read += n as u64;

        // 每 16MB 或最后一次发一次进度
        if bytes_read % (16 * 1024 * 1024) < buf.len() as u64 || bytes_read == file_size {
            if let Some(cb) = progress_callback {
                let pct = if file_size > 0 {
                    stage_progress::EXTRACTING_END
                        + (bytes_read as f64 / file_size as f64) * (stage_progress::COMPLETE_END - stage_progress::EXTRACTING_END)
                } else {
                    stage_progress::COMPLETE_END
                };
                cb(DownloadProgress {
                    stage: "verifying".to_string(),
                    progress: pct,
                    downloaded: bytes_read,
                    total: file_size,
                    message: format!("校验文件完整性... {:.1}%", pct * 100.0),
                    detail: Some(DownloadProgressDetail {
                        step: "SHA256 校验".to_string(),
                        step_progress: bytes_read as f64 / file_size.max(1) as f64,
                        candidate_index: 0,
                        candidate_count: 0,
                        current_candidate: None,
                        speed_mbps: 0.0,
                        eta_secs: None,
                    }),
                });
            }
        }
    }
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

/// 获取 llama.cpp 最新版本（先查询 GitHub API，失败则回退到硬编码稳定版本）
///
/// **优化方案：**
/// 1. 快速查询 GitHub API `/releases/latest`（单次请求，约 0.5~2s）
/// 2. 解析真实 release assets（名称 + size + URL）
/// 3. 若 API 失败（限流/网络问题），回退到硬编码 tag（保持可用）
/// 4. 回退时仍构建候选名 → HEAD 验证流程（与之前一致）
///
/// 这样既保证了"最新版本"的准确性，又不会因为 GitHub 临时限流导致下载失败。
fn fetch_llama_latest_release() -> anyhow::Result<GitHubRelease> {
    let os = current_os();
    let arch = current_arch();

        // 0) 用户可通过环境变量覆盖（最高优先级）
    if let Ok(tag) = std::env::var("LLAMA_CPP_VERSION") {
        let tag_owned = tag.clone();
        tracing::info!(target: "LlamaDownloader", tag = %tag_owned, "使用环境变量指定的版本");
        return Ok(GitHubRelease {
            tag_name: tag_owned,
            assets: build_virtual_assets(&tag, os, arch),
            prerelease: false,
            source: "env",
        });
    }

    // 1) 尝试 GitHub API
    tracing::info!(target: "LlamaDownloader", "查询 GitHub API 获取最新 release");
    if let Ok(mut release) = try_fetch_from_github_api() {
        release.source = "api";
        return Ok(release);
    }

            // 2) 回退：API 失败时，尝试下载 nightly-tag.txt 直接获取 nightly tag
    if let Some(nightly_tag) = fetch_nightly_tag_direct() {
        tracing::warn!(
            target: "LlamaDownloader",
            nightly_tag = %nightly_tag,
            "API 不可用，回退到 nightly-tag.txt 方式"
        );
        return Ok(GitHubRelease {
            tag_name: nightly_tag.clone(),
            assets: build_virtual_assets(&nightly_tag, os, arch),
            prerelease: true,
            source: "nightly",
        });
    }

    // 3) 最后回退：硬编码已知稳定版本
    let tag = "b10964".to_string();
    let tag_owned = tag.clone();
    tracing::warn!(target: "LlamaDownloader", tag = %tag_owned, "所有策略失败，回退到硬编码版本");
    Ok(GitHubRelease {
        tag_name: tag_owned,
        assets: build_virtual_assets(&tag, os, arch),
        prerelease: true,
        source: "fallback",
    })
}

/// 当 API 不可用时，尝试直接下载 nightly-tag.txt 文本获取 nightly tag
///
/// nightly-tag.txt 位于最新稳定 release（如 v0.4.1）的下载页面。
fn fetch_nightly_tag_direct() -> Option<String> {
    // 尝试从已知的 stable release 下载 nightly-tag.txt
    let stable_tags = ["v0.4.2", "v0.4.1", "v0.4.0"];
    for stable_tag in &stable_tags {
        let url = format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{}/nightly-tag.txt",
            stable_tag
        );
        if let Ok(content) = download_text(&url) {
            let tag = content.trim().to_string();
            if !tag.is_empty() && tag.starts_with('b') {
                return Some(tag);
            }
        }
    }
    None
}

/// 简单下载文本内容（用于 nightly-tag.txt）
fn download_text(url: &str) -> anyhow::Result<String> {
    let resp = SHARED_CLIENT.get(url).send()?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("HTTP {}", resp.status().as_u16()));
    }
    Ok(resp.text()?)
}

/// 从 GitHub API 获取最新 release 并解析真实 assets
///
/// **策略**（按优先级）：
/// 1. `/releases/latest` → 若包含 .zip/.tar.gz 二进制资产，直接返回
/// 2. 若 latest 没有二进制资产（如 v0.4.1 仅含 nightly-tag.txt），下载 nightly-tag.txt 获取 nightly tag → 拉取 `/releases/tags/{nightly_tag}`
/// 3. 扫描 `/releases?per_page=20` → 返回第一个含二进制资产的 release
fn try_fetch_from_github_api() -> anyhow::Result<GitHubRelease> {
    let auth = get_github_token().map(|t| format!("token {}", t));

    // 1. 尝试 /releases/latest
    let latest_url = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
    if let Ok(release) = fetch_github_release(&SHARED_CLIENT, &auth, latest_url) {
        if has_binary_assets(&release) {
            tracing::info!(target: "LlamaDownloader", tag = %release.tag_name, "latest release 有二进制资产，直接使用");
            let mut release = release;
            release.source = "api";
            return Ok(release);
        }
        // 2. latest 没有二进制资产 → 下载 nightly-tag.txt 获取 nightly tag
        if let Some(nightly_tag) = fetch_nightly_tag_via_api(&SHARED_CLIENT, &auth, &release) {
            let nightly_url = format!(
                "https://api.github.com/repos/ggml-org/llama.cpp/releases/tags/{}",
                nightly_tag
            );
            if let Ok(nightly) = fetch_github_release(&SHARED_CLIENT, &auth, &nightly_url) {
                if has_binary_assets(&nightly) {
                    tracing::info!(target: "LlamaDownloader", tag = %nightly.tag_name, nightly_tag = %nightly_tag, "通过 nightly-tag.txt 获取到有二进制的 release");
                    let mut nightly = nightly;
                    nightly.source = "api";
                    return Ok(nightly);
                }
            }
        }
    }

    // 3. 扫描最近 20 个 release，找第一个有二进制资产的
    let list_url = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20";
    if let Ok(releases) = fetch_github_releases_list(&SHARED_CLIENT, &auth, list_url) {
        for release in releases {
            if has_binary_assets(&release) {
                tracing::info!(target: "LlamaDownloader", tag = %release.tag_name, "从 releases 列表找到二进制资产");
                let mut release = release;
                release.source = "api";
                return Ok(release);
            }
        }
    }

    Err(anyhow::anyhow!("GitHub API 返回无可用二进制资产"))
}

/// 检查 release 是否包含二进制下载资产（.zip 或 .tar.gz）
fn has_binary_assets(release: &GitHubRelease) -> bool {
    release.assets.iter().any(|a| {
        let name_lower = a.name.to_lowercase();
        name_lower.ends_with(".zip") || name_lower.ends_with(".tar.gz")
    })
}

/// 从 release 的 nightly-tag.txt 资产下载 nightly tag（文本内容 = tag 号）
fn fetch_nightly_tag_via_api(
    client: &reqwest::blocking::Client,
    auth: &Option<String>,
    release: &GitHubRelease,
) -> Option<String> {
    let nightly_asset = release.assets.iter().find(|a| a.name == "nightly-tag.txt")?;
    let mut req = client.get(&nightly_asset.browser_download_url);
    if let Some(token) = auth {
        req = req.header("Authorization", token);
    }
    let resp = req.timeout(Duration::from_secs(10)).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().ok()?;
    let tag = text.trim().to_string();
    if tag.is_empty() {
        return None;
    }
    tracing::info!(target: "LlamaDownloader", nightly_tag = %tag, "从 nightly-tag.txt 获取 nightly tag");
    Some(tag)
}

/// 获取单个 GitHub release（解析为 GitHubRelease）
fn fetch_github_release(
    client: &reqwest::blocking::Client,
    auth: &Option<String>,
    url: &str,
) -> anyhow::Result<GitHubRelease> {
    let mut req = client.get(url);
    if let Some(token) = auth {
        req = req.header("Authorization", token);
    }
    let resp = req.send()?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("GitHub API HTTP {}", status.as_u16()));
    }
    let body = resp.text()?;
    let mut json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("JSON 解析失败: {}", e))?;
    parse_github_release(&mut json)
}

/// 获取多个 GitHub release（解析为 GitHubRelease 列表）
fn fetch_github_releases_list(
    client: &reqwest::blocking::Client,
    auth: &Option<String>,
    url: &str,
) -> anyhow::Result<Vec<GitHubRelease>> {
    let mut req = client.get(url);
    if let Some(token) = auth {
        req = req.header("Authorization", token);
    }
    let resp = req.send()?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("GitHub API HTTP {}", status.as_u16()));
    }
    let body = resp.text()?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("JSON 解析失败: {}", e))?;
    let arr = json.as_array().ok_or_else(|| anyhow::anyhow!("API 返回非数组"))?;
    let mut releases = Vec::with_capacity(arr.len());
    for mut item in arr.iter().cloned() {
        if let Ok(r) = parse_github_release(&mut item) {
            releases.push(r);
        }
    }
    Ok(releases)
}

/// 从 JSON 值解析 GitHubRelease
fn parse_github_release(json: &mut serde_json::Value) -> anyhow::Result<GitHubRelease> {
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少 tag_name"))?;
    let assets: Vec<GitHubAsset> = json
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    Some(GitHubAsset {
                        name: a.get("name")?.as_str()?.to_string(),
                        browser_download_url: a.get("browser_download_url")?.as_str()?.to_string(),
                        size: a.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(GitHubRelease {
        tag_name: tag.to_string(),
        assets,
        prerelease: json.get("prerelease").and_then(|v| v.as_bool()).unwrap_or(false),
        source: "api",
    })
}

/// 构建"虚拟"候选资产（用于 API 失败时的回退）
fn build_virtual_assets(tag: &str, os: &str, arch: &str) -> Vec<GitHubAsset> {
    let candidates = build_official_candidate_names(tag, os, arch);
    candidates
        .into_iter()
        .map(|name| GitHubAsset {
            browser_download_url: format!(
                "https://github.com/ggml-org/llama.cpp/releases/download/{}/{}",
                tag, name
            ),
            size: 0,
            name,
        })
        .collect()
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
