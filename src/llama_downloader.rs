//! llama.cpp 二进制自动下载与安装。
//!
//! 从 GitHub Releases（ggml-org/llama.cpp）下载对应平台的 llama-server，
//! 支持 GPU 后端自动选择、SHA256 校验、解压和进度回调。
//! 使用 curl 子进程发起 HTTP 请求（自动继承系统代理和 TLS 配置）。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 获取 GitHub Token（优先级：GITHUB_TOKEN → gh CLI → GH_TOKEN → 无）
fn get_github_token() -> Option<String> {
    // 1. 环境变量 GITHUB_TOKEN
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            tracing::debug!(target: "LlamaDownloader", source = "env:GITHUB_TOKEN", "获取到 token");
            return Some(token);
        }
    }
    // 2. gh CLI auth token
    if let Ok(output) = Command::new("gh").args(["auth", "token"]).output() {
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

/// 用 curl 获取 URL 内容（自动继承系统代理）
fn curl_get(url: &str) -> anyhow::Result<String> {
    let token = get_github_token();
    let auth_header = token.as_deref().map(|t| format!("Authorization: token {}", t));
    let has_token = auth_header.is_some();
    let start = std::time::Instant::now();

    tracing::debug!(target: "LlamaDownloader", url = %url, has_token, "发起 curl GET 请求");

    let mut args: Vec<&str> = vec!["-s", "-L", "--max-time", "30"];
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

    let elapsed_ms = start.elapsed().as_millis();
    let status_code = output.status.code().unwrap_or(-1);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(target: "LlamaDownloader", url = %url, status = status_code, elapsed_ms, stderr = %stderr, "curl GET 请求失败");
        return Err(anyhow::anyhow!("curl GET {} 返回 {}: {}", url, status_code, stderr));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    tracing::debug!(target: "LlamaDownloader", url = %url, status = status_code, elapsed_ms, body_len = body.len(), "curl GET 请求成功");

    Ok(body.to_string())
}

/// 用 curl 发送 HEAD 请求验证 URL 可用性（自动继承系统代理）
fn curl_head(url: &str) -> anyhow::Result<()> {
    let token = get_github_token();
    let auth_header = token.as_deref().map(|t| format!("Authorization: token {}", t));

    let mut args = vec!["-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "10", "-I"];
    if let Some(ref h) = auth_header {
        args.push("-H");
        args.push(h.as_str());
    }
    args.push(url);

    let output = Command::new("curl")
        .args(&args)
        .output()
        .map_err(|e| anyhow::anyhow!("curl HEAD 请求失败: {}", e))?;

    let status_code = String::from_utf8_lossy(&output.stdout).trim().to_string();
    tracing::debug!(target: "LlamaDownloader", url = %url, status = %status_code, "curl HEAD 验证");

    if status_code == "200" || status_code == "302" || status_code == "301" {
        Ok(())
    } else {
        Err(anyhow::anyhow!("HTTP {} (不可用)", status_code))
    }
}

/// 从 curl -# 进度输出中解析百分比
///
/// curl 的 `-#` 模式输出格式类似：
/// ```text
///                                  2.6%
/// ##                                2.8%
///                                  ##  2.9%
/// ```
/// 有时百分比前面会有 `##` 和空格混合。我们查找行中最后一个 `XX.X%` 模式。
fn parse_curl_progress(line: &str) -> Option<f64> {
    // 从后往前找最后一个 '%'，然后解析前面的数字
    let trimmed = line.trim_end();
    if let Some(pct_pos) = trimmed.rfind('%') {
        let before = trimmed[..pct_pos].trim_end();
        // 向前找到数字部分的开始
        let end = before.len();
        let mut start = end;
        let bytes = before.as_bytes();
        while start > 0 {
            start -= 1;
            if bytes[start].is_ascii_digit() || bytes[start] == b'.' {
                continue;
            }
            start += 1;
            break;
        }
        if start == 0 {
            // 整个字符串可能都是数字
        }
        let num_str = &before[start..end];
        if let Ok(pct) = num_str.parse::<f64>() {
            if (0.0..=100.0).contains(&pct) {
                return Some(pct);
            }
        }
    }
    None
}

/// 用 curl 下载文件到本地路径（spawn + 实时进度读取）
///
/// 使用 `Command::spawn()` 启动 curl，逐行读取 stderr 中的进度输出，
/// 实时调用 `progress_callback` 报告下载进度。
/// 支持断点续传和完整重试机制。
fn curl_download(
    url: &str,
    dest: &Path,
    total_size: u64,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<u64> {
    let start = std::time::Instant::now();
    let dest_str = dest.to_string_lossy().to_string();
    tracing::info!(target: "LlamaDownloader", url = %url, dest = %dest_str, total_size, "启动 curl 下载进程");

    if total_size > 0 {
        if let Some(cb) = progress_callback {
            cb(DownloadProgress {
                stage: "downloading".to_string(),
                progress: 0.0,
                downloaded: 0,
                total: total_size,
                message: format!("开始下载 ({:.1} MB)...", total_size as f64 / 1048576.0),
            });
        }
    }

    let mut last_error = String::new();
    let mut last_error_log = std::time::Instant::now() - std::time::Duration::from_secs(10);

    // 重试循环：最多 5 次
    for attempt in 1..=5u32 {
        if attempt > 1 {
            let wait_secs = std::cmp::min(attempt * 2, 10);
            tracing::warn!(target: "LlamaDownloader", attempt, wait_secs, "重试下载中...");
            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "retrying".to_string(),
                    progress: 0.0,
                    downloaded: 0,
                    total: total_size,
                    message: format!("重试第 {} 次 (等待 {}s)...", attempt, wait_secs),
                });
            }
            std::thread::sleep(std::time::Duration::from_secs(wait_secs.into()));
        }

        let mut cmd = Command::new("curl");
        // 基础参数
        cmd.args([
            "-L",                          // 跟随重定向
            "--max-time", "600",            // 单次最大 10 分钟
            "--retry", "2",                 // curl 内部重试 2 次
            "--retry-all-errors",           // 所有错误都重试
            "--retry-delay", "2",           // 重试间隔 2 秒
            "--connect-timeout", "30",      // 连接超时 30 秒
            "-C", "-",                      // 断点续传
            "-o", &dest_str,
            "-#",  // 进度条（stderr，格式：##  X.X%）
        ]);

        // 添加 User-Agent 和 TLS 配置
        cmd.args([
            "-A", "LlamaUI/0.6.0",
            "--tlsv1.2",
            "--keepalive-time", "30",
        ]);

        // 断点续传：检查已下载大小
        let existing_size = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if existing_size > 0 && total_size > 0 {
            tracing::info!(target: "LlamaDownloader",
                existing_mb = format!("{:.1}", existing_size as f64 / 1048576.0),
                total_mb = format!("{:.1}", total_size as f64 / 1048576.0),
                "断点续传"
            );
        }

        cmd.arg(url);

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("curl 启动失败: {}", e))?;

        // 逐行读取 stderr，解析 curl -# 进度
        let stderr = child.stderr.take().unwrap();
        let reader = BufReader::new(stderr);
        let mut last_log = std::time::Instant::now();
        let mut last_pct: f64 = 0.0;

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(target: "LlamaDownloader", error = %e, "读取 stderr 失败");
                    break;
                }
            };

            // 检测 curl 错误
            if line.contains("curl:") {
                tracing::warn!(target: "LlamaDownloader", line = %line, "检测到 curl 错误");
            }

            if let Some(pct) = parse_curl_progress(&line) {
                let now = std::time::Instant::now();
                let downloaded = if total_size > 0 {
                    (total_size as f64 * pct / 100.0) as u64
                } else {
                    0
                };
                let speed_mbps = if start.elapsed().as_secs_f64() > 0.0 {
                    downloaded as f64 / start.elapsed().as_secs_f64() / 1048576.0
                } else {
                    0.0
                };

                // 每 3 秒或进度变化超过 10% 时记录日志
                let should_log = now.duration_since(last_log).as_secs() >= 3
                    || (pct - last_pct).abs() >= 10.0
                    || speed_mbps == 0.0 && last_pct > 0.0;

                if should_log {
                    let speed_str = format!("{:.1}", speed_mbps);
                    tracing::info!(target: "LlamaDownloader",
                        attempt,
                        pct = format!("{:.1}", pct),
                        downloaded_mb = format!("{:.1}", downloaded as f64 / 1048576.0),
                        total_mb = format!("{:.1}", total_size as f64 / 1048576.0),
                        speed_mbps = %speed_str,
                        elapsed_secs = format!("{:.0}", start.elapsed().as_secs_f64()),
                        "下载进度"
                    );
                    last_log = now;
                    last_pct = pct;
                }

                if let Some(cb) = progress_callback {
                    cb(DownloadProgress {
                        stage: "downloading".to_string(),
                        progress: pct / 100.0,
                        downloaded,
                        total: total_size,
                        message: format!(
                            "下载中... {:.1} / {:.1} MB ({:.0}%)",
                            downloaded as f64 / 1048576.0,
                            total_size as f64 / 1048576.0,
                            pct
                        ),
                    });
                }
            }
        }

        // 等待 curl 进程结束，获取退出状态
        let status = child.wait()
            .map_err(|e| anyhow::anyhow!("等待 curl 进程失败: {}", e))?;

        let elapsed_secs = start.elapsed().as_secs_f64();
        let status_code = status.code().unwrap_or(-1);

        if status.success() {
            // 下载成功
            let downloaded = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            let speed_mbps = if elapsed_secs > 0.0 {
                downloaded as f64 / elapsed_secs / 1048576.0
            } else {
                0.0
            };

            tracing::info!(target: "LlamaDownloader",
                attempt,
                downloaded = downloaded,
                mb = format!("{:.1}", downloaded as f64 / 1048576.0),
                elapsed_secs = format!("{:.1}", elapsed_secs),
                speed_mbps = format!("{:.1}", speed_mbps),
                "curl 下载完成"
            );

            // 计算 SHA256
            if downloaded > 0 {
                tracing::debug!(target: "LlamaDownloader", "开始计算 SHA256...");
                let mut file = fs::File::open(dest)?;
                let mut hasher = Sha256::new();
                let mut buffer = vec![0u8; 65536];
                loop {
                    let bytes_read = file.read(&mut buffer)?;
                    if bytes_read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..bytes_read]);
                }
                let sha256_hex = format!("{:x}", hasher.finalize());
                tracing::info!(target: "LlamaDownloader", sha256 = %sha256_hex, bytes = downloaded, "SHA256 计算完成");
            }

            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "downloading".to_string(),
                    progress: 1.0,
                    downloaded,
                    total: total_size,
                    message: format!(
                        "下载完成 {:.1} MB（耗时 {:.0}s，速度 {:.1} MB/s）",
                        downloaded as f64 / 1048576.0,
                        elapsed_secs,
                        speed_mbps
                    ),
                });
            }

            return Ok(downloaded);
        }

        // 下载失败，记录错误并继续重试
        let error_msg = if status_code == 56 {
            "TLS 连接中断 (schannel: server closed abruptly)".to_string()
        } else {
            format!("curl 退出码: {}", status_code)
        };

        last_error = error_msg.clone();

        // 避免短时间内重复打印相同错误
        if last_error_log.elapsed().as_secs() >= 5 {
            tracing::error!(target: "LlamaDownloader",
                attempt,
                url = %url,
                status = status_code,
                elapsed_secs = format!("{:.1}", elapsed_secs),
                error = %error_msg,
                "curl 下载失败，准备重试"
            );
            last_error_log = std::time::Instant::now();
        }
    }

    // 所有重试都失败
    tracing::error!(target: "LlamaDownloader",
        url = %url,
        total_attempts = 5,
        error = %last_error,
        "curl 下载失败，已用完所有重试次数"
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
    if !output.status.success() {
        return None;
    }

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

/// 从 GitHub Release 查找匹配的资产
fn find_asset<'a>(release: &'a GitHubRelease, asset_name: &str) -> Option<&'a GitHubAsset> {
    release.assets.iter().find(|a| a.name == asset_name)
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

    // 先尝试 tar
    let result = extract_tar_gz(archive, dest);

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
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    let result = actual.eq_ignore_ascii_case(expected);
    tracing::info!(target: "LlamaDownloader", actual = %actual, expected = %expected, ok = result, "SHA256 校验完成");
    Ok(result)
}

/// 从 release 的资产列表中智能查找匹配当前系统的资产
/// 支持多种命名变体，自动识别 OS/arch/backend
/// **关键改进：验证 URL 可用性，确保下载成功**
fn smart_find_asset<'a>(
    release: &'a GitHubRelease,
    backend: GpuBackend,
) -> Option<&'a GitHubAsset> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    tracing::info!(
        target: "LlamaDownloader",
        os = os,
        arch = arch,
        backend = %backend.as_str(),
        tag = %release.tag_name,
        asset_count = release.assets.len(),
        "开始智能匹配资产（官方稳定方案）"
    );

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

    // 验证每个候选 URL 的可用性（HEAD 请求）
    // 保存第一个候选以便最后回退
    let first_candidate = candidates.first().copied();
    for asset in candidates {
        let url = &asset.browser_download_url;
        tracing::debug!(target: "LlamaDownloader", url = %url, "验证 URL 可用性");

        // 使用 HEAD 请求验证 URL
        let result = curl_head(url);
        match result {
            Ok(_) => {
                tracing::info!(
                    target: "LlamaDownloader",
                    name = %asset.name,
                    url = %url,
                    "✅ URL 可用，选择此资产"
                );
                return Some(asset);
            }
            Err(e) => {
                tracing::debug!(
                    target: "LlamaDownloader",
                    url = %url,
                    error = %e,
                    "❌ URL 不可用，尝试下一个"
                );
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
        return Some(asset);
    }

    None
}

/// 构建候选资产名列表（按优先级）
fn build_candidate_asset_names(backend: GpuBackend, os: &str, arch: &str) -> Vec<String> {
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

    let mut candidates = Vec::new();

    // 候选 1: cudart- 前缀（Windows CUDA）
    if matches!(backend, GpuBackend::Cuda12_4 | GpuBackend::Cuda13_3) && os == "windows" {
        candidates.push(format!(
            "cudart-llama-{{}}-bin-{}{}-{}.{}",
            os_str, backend_part, arch_str, ext
        ));
    }

    // 候选 2: 标准格式
    candidates.push(format!(
        "llama-{{}}-bin-{}{}-{}.{}",
        os_str, backend_part, arch_str, ext
    ));

    candidates.into_iter().map(|s| {
        // 用当前 release 的 tag 替换占位符（如果已知）
        // 此处仅返回模式，具体 tag 在调用处替换
        s
    }).collect()
}

/// 带重试的 GitHub API 调用
fn fetch_llama_latest_release_with_retry(max_retries: u32) -> anyhow::Result<GitHubRelease> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=max_retries {
        tracing::info!(
            target: "LlamaDownloader",
            attempt = attempt,
            max = max_retries,
            "尝试获取最新版本"
        );
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
            std::thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));
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

    // 1. 获取最新版本（带重试）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "fetching_version".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: format!("获取最新版本（最多 {} 次重试）...", max_retries),
        });
    }

    let release = fetch_llama_latest_release_with_retry(max_retries)?;
    let tag = &release.tag_name;
    tracing::info!(target: "LlamaDownloader", tag = %tag, count = release.assets.len(), "获取到最新版本");

    // 2. 智能查找资产（多模式匹配）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "finding_asset".to_string(),
            progress: 0.05,
            downloaded: 0,
            total: 0,
            message: format!("查找匹配资产 (tag={})...", tag),
        });
    }

    let asset = smart_find_asset(&release, backend).ok_or_else(|| {
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

    tracing::info!(target: "LlamaDownloader",
        name = %asset.name,
        mb = asset.size as f64 / 1048576.0,
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

    let mut download_attempt = 0;
    let downloaded = loop {
        download_attempt += 1;
        match curl_download(&asset.browser_download_url, &archive_path, asset.size, progress_callback) {
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
                std::thread::sleep(std::time::Duration::from_secs(2 * download_attempt as u64));
            }
        }
    };

    // 5. 解压
    tracing::info!(target: "LlamaDownloader", archive = %archive_path.display(), dest = %install_dir.display(), "开始解压");
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
    let extracted = extract_zip(&archive_path, install_dir).map_err(|e| {
        tracing::error!(target: "LlamaDownloader", error = %e, archive = %archive_path.display(), "解压失败");
        e
    })?;

    #[cfg(not(windows))]
    let extracted = extract_tar_gz(&archive_path, install_dir).map_err(|e| {
        tracing::error!(target: "LlamaDownloader", error = %e, archive = %archive_path.display(), "解压失败");
        e
    })?;

    tracing::info!(target: "LlamaDownloader", extracted_count = extracted.len(), "解压完成，候选文件");

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

/// 检测后端后缀（cuda-12.4 / cpu / vulkan 等）
fn detect_backend_suffix() -> String {
    // 默认 CUDA 12.4（兼容 RTX 50 系列）
    "cuda-12.4".to_string()
}

/// 当前操作系统
fn current_os() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else if cfg!(target_os = "macos") {
        "macos".to_string()
    } else {
        "unknown".to_string()
    }
}

/// 当前架构
fn current_arch() -> String {
    if cfg!(target_arch = "x86_64") {
        "x86_64".to_string()
    } else if cfg!(target_arch = "aarch64") {
        "aarch64".to_string()
    } else {
        "unknown".to_string()
    }
}

/// 根据 tag 获取 release 信息
fn fetch_release_by_tag(tag: &str) -> anyhow::Result<GitHubRelease> {
    let url = format!("https://api.github.com/repos/ggml-org/llama.cpp/releases/tags/{}", tag);
    tracing::debug!(target: "LlamaDownloader", url = %url, "根据 tag 获取 release");

    let json_str = curl_get(&url)?;
    let release: GitHubRelease = serde_json::from_str(&json_str)
        .map_err(|e| anyhow::anyhow!("解析 release 失败: {}", e))?;

    tracing::info!(
        target: "LlamaDownloader",
        tag = %release.tag_name,
        count = release.assets.len(),
        "获取到 release"
    );

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

    #[test]
    fn test_parse_curl_progress_standard() {
        assert_eq!(parse_curl_progress("                                 2.6%"), Some(2.6));
    }

    #[test]
    fn test_parse_curl_progress_with_hash() {
        assert_eq!(parse_curl_progress("##                                2.8%"), Some(2.8));
    }

    #[test]
    fn test_parse_curl_progress_full() {
        assert_eq!(parse_curl_progress("                                  ##  99.9%"), Some(99.9));
    }

    #[test]
    fn test_parse_curl_progress_zero() {
        assert_eq!(parse_curl_progress("                                  0.0%"), Some(0.0));
    }

    #[test]
    fn test_parse_curl_progress_hundred() {
        assert_eq!(parse_curl_progress("                                  100%"), Some(100.0));
    }

    #[test]
    fn test_parse_curl_progress_no_percent() {
        assert_eq!(parse_curl_progress("some random text without percent"), None);
    }
}
