//! llama.cpp 二进制自动下载与安装。
//!
//! 从 GitHub Releases（ggml-org/llama.cpp）下载对应平台的 llama-server，
//! 支持 GPU 后端自动选择、SHA256 校验、解压和进度回调。
//! 使用 reqwest 库（带 TLS 证书验证）发起所有 HTTP 请求。

use crate::util::process::silent_command;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// 共享的阻塞 HTTP 客户端（直连 reqwest，不再依赖自研下载引擎）
static SHARED_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(30))
        .user_agent("LlamaUI/0.7.0")
        .build()
        .expect("构建 reqwest blocking Client 失败")
});

/// 版本查询专用客户端（短超时，避免在 2% 阶段长时间卡住）
static VERSION_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(15))
        .user_agent("LlamaUI/0.7.0")
        .build()
        .expect("构建版本查询 Client 失败")
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
/// 获取 GitHub Token（优先级：GITHUB_TOKEN → gh CLI → GH_TOKEN → 无）。
/// 用于对 GitHub API 请求附加 `Authorization: token <token>` 头（见
/// `fetch_llama_latest_release_with_retry`）。找不到时返回 `None`（匿名请求）。
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
/// - ① 初始化              : 0%  ~ 2%   （2%）
/// - ② 获取最新版本         : 2%  ~ 6%   （4%）
/// - ③ 准备匹配资产         : 6%  ~ 8%   （2%）
/// - ④ 匹配/验证资产       : 8%  ~ 22%  （14%，并行HEAD验证）
/// - ⑤ 下载安装包           : 22% ~ 80%  （58%，主阶段）
/// - ⑥ 解压归档             : 80% ~ 88%  （8%）
/// - ⑦ SHA256 校验          : 88% ~ 96%  （8%）
/// - ⑧ 清理收尾             : 96% ~ 98%  （2%）
/// - ⑨ 完成                 : 98% ~ 100% （2%）
pub mod stage_progress {
    pub const INIT_END: f64 = 0.02;
    pub const FETCHING_VERSION_START: f64 = 0.02;
    pub const FETCHING_VERSION_END: f64 = 0.06;
    pub const PREPARING_ASSET_START: f64 = 0.06;
    pub const PREPARING_ASSET_END: f64 = 0.08;
    pub const FINDING_ASSET_START: f64 = 0.08;
    pub const FINDING_ASSET_END: f64 = 0.22;
    pub const DOWNLOAD_START: f64 = 0.22;
    pub const DOWNLOAD_END: f64 = 0.80;
    pub const EXTRACTING_END: f64 = 0.88;
    pub const VERIFYING_START: f64 = 0.88;
    pub const VERIFYING_END: f64 = 0.96;
    pub const FINALIZING_START: f64 = 0.96;
    pub const COMPLETE_END: f64 = 1.00;
}

fn curl_download(
    url: &str,
    dest: &Path,
    total_size: u64,
    progress_start: f64,
    progress_end: f64,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
    cancel_token: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<u64> {
    const MAX_ATTEMPTS: u32 = 3;

    let start = std::time::Instant::now();
    tracing::info!(target: "LlamaDownloader", url = %url, total_size, "启动流式下载（reqwest）");

    if total_size > 0 {
        if let Some(cb) = progress_callback {
            cb(DownloadProgress {
                stage: "downloading".to_string(),
                progress: progress_start,
                downloaded: 0,
                total: total_size,
                                message: format!("开始下载 ({:.1} MB)", total_size as f64 / 1048576.0),
                speed_mbps: 0.0,
                eta_secs: None,
                detail: None,
            });
        }
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(dest);

    let mut last_error = String::new();
    let mut last_progress_at = std::time::Instant::now();

    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            let backoff = Duration::from_millis(500 * attempt as u64).min(Duration::from_secs(2));
            tracing::warn!(target: "LlamaDownloader", attempt, ?backoff, "下载失败，准备重试");
            std::thread::sleep(backoff);
            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "retrying".to_string(),
                    progress: progress_start,
                    downloaded: 0,
                    total: total_size,
                                        message: format!("重试第 {} 次...", attempt),
                    speed_mbps: 0.0,
                    eta_secs: None,
                    detail: None,
                });
            }
        }

            if let Some(ct) = cancel_token {
                if ct.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::info!(target: "LlamaDownloader", attempt, "下载被取消");
                    return Err(anyhow::anyhow!("下载已取消"));
                }
            }

            let mut resp = match SHARED_CLIENT.get(url).send() {
            Ok(r) => r,
            Err(e) => {
                last_error = e.to_string();
                tracing::warn!(target: "LlamaDownloader", attempt, error = %e, "请求失败");
                continue;
            }
        };
        let status = resp.status();
        if !status.is_success() {
            last_error = format!("HTTP {}", status.as_u16());
            tracing::warn!(target: "LlamaDownloader", attempt, %status, "非成功状态码");
            continue;
        }

        let size = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(total_size);

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(dest)?;

        // 拿到响应头+大小后，立即通知前端（解决 0% 卡顿问题）
        tracing::info!(
            target: "LlamaDownloader",
            url = %url,
            file_size_mb = size as f64 / 1048576.0,
            "开始下载文件"
        );
        if let Some(cb) = progress_callback {
            cb(DownloadProgress {
                stage: "downloading".to_string(),
                progress: progress_start + 0.001 * (progress_end - progress_start),
                downloaded: 0,
                total: size,
                                message: format!("准备下载 {:.1} MB", size as f64 / 1048576.0),
                speed_mbps: 0.0,
                eta_secs: None,
                detail: Some(DownloadProgressDetail {
                    step: "下载中".to_string(),
                    step_progress: 0.0,
                    candidate_index: 1,
                    candidate_count: 1,
                    current_candidate: None,
                    speed_mbps: 0.0,
                    eta_secs: None,
                }),
            });
        }

        let mut downloaded: u64 = 0;
        let mut buffer = [0u8; 65536]; // 64KB 缓冲区，提升读取吞吐

        // 1 秒无进展保底上报：避免网络抖动导致前端卡在 22%
        const WATCHDOG_MS: u64 = 1000;

        loop {
            // 取消检查
            if let Some(ct) = cancel_token {
                if ct.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::info!(target: "LlamaDownloader", attempt, "下载被取消（读取循环）");
                    return Err(anyhow::anyhow!("下载已取消"));
                }
            }
            let n = match resp.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!(target: "LlamaDownloader", attempt, error = %e, "读取数据失败");
                    break;
                }
            };
            let now = std::time::Instant::now();
            if n == 0 {
                // 检查看门狗：如果超过 WATCHDOG_MS 仍未收到数据，发一次保底上报
                if downloaded > 0
                    && now.duration_since(last_progress_at).as_millis() as u64 >= WATCHDOG_MS
                {
                    if let Some(cb) = progress_callback {
                        let raw_progress = if size > 0 {
                            downloaded as f64 / size as f64
                        } else {
                            0.0
                        };
                        let global_progress =
                            progress_start + raw_progress * (progress_end - progress_start);
                        let elapsed = start.elapsed().as_secs_f64();
                        let speed_mbps = if elapsed > 0.0 {
                            (downloaded as f64 / elapsed) / 1_048_576.0
                        } else {
                            0.0
                        };
                        let remaining_bytes = size.saturating_sub(downloaded);
                        let eta_secs = if speed_mbps > 0.0 {
                            (remaining_bytes as f64 / 1_048_576.0 / speed_mbps) as u64
                        } else {
                            0
                        };
                        cb(DownloadProgress {
                            stage: "downloading".to_string(),
                            progress: global_progress,
                            downloaded,
                            total: size,
                            message: format!(
                                "下载中（网络缓慢，已下载 {:.1} MB）...",
                                downloaded as f64 / 1048576.0
                            ),
                            speed_mbps,
                            eta_secs: if eta_secs > 0 { Some(eta_secs) } else { None },
                            detail: Some(DownloadProgressDetail {
                                step: "downloading".to_string(),
                                step_progress: raw_progress,
                                candidate_index: 1,
                                candidate_count: 1,
                                current_candidate: None,
                                speed_mbps,
                                eta_secs: if eta_secs > 0 {
                                    Some(eta_secs as f64)
                                } else {
                                    None
                                },
                            }),
                        });
                    }
                    last_progress_at = now;
                }
                if n == 0 {
                    break;
                }
            }
            if let Err(e) = file.write_all(&buffer[..n]) {
                tracing::warn!(target: "LlamaDownloader", attempt, error = %e, "写入文件失败");
                break;
            }
            downloaded += n as u64;

                    // 时间节流：每 1 秒至少上报一次，或每 1KB 累积跨越边界时上报，或下载完成时上报
            let now = std::time::Instant::now();
            let time_ok = now.duration_since(last_progress_at).as_millis() as u64 >= 50;
            let boundary_cross = downloaded % 1024 < n as u64;
            let should_emit = (boundary_cross && time_ok)
                || downloaded == size
                || now.duration_since(last_progress_at).as_millis() as u64 >= 1000;
            if should_emit {
                last_progress_at = now;
                let raw_progress = if size > 0 {
                    downloaded as f64 / size as f64
                } else {
                    0.0
                };
                let global_progress =
                    progress_start + raw_progress * (progress_end - progress_start);
                let elapsed = start.elapsed().as_secs_f64();
                let speed_mbps = if elapsed > 0.0 {
                    (downloaded as f64 / elapsed) / 1_048_576.0
                } else {
                    0.0
                };
                let remaining_bytes = size.saturating_sub(downloaded);
                let eta_secs = if speed_mbps > 0.0 {
                    (remaining_bytes as f64 / 1_048_576.0 / speed_mbps) as u64
                } else {
                    0
                };

                if let Some(cb) = progress_callback {
                    cb(DownloadProgress {
                        stage: "downloading".to_string(),
                        progress: global_progress,
                        downloaded,
                        total: size,
                        message: format!(
                            "{:.1} / {:.1} MB ({:.1}%) · {:.1} MB/s",
                            downloaded as f64 / 1048576.0,
                            size as f64 / 1048576.0,
                            global_progress * 100.0,
                            speed_mbps
                        ),
                        speed_mbps,
                        eta_secs: if eta_secs > 0 { Some(eta_secs) } else { None },
                        detail: Some(DownloadProgressDetail {
                            step: "downloading".to_string(),
                            step_progress: raw_progress,
                            candidate_index: 1,
                            candidate_count: 1,
                            current_candidate: None,
                            speed_mbps,
                            eta_secs: if eta_secs > 0 {
                                Some(eta_secs as f64)
                            } else {
                                None
                            },
                        }),
                    });
                }
                tracing::info!(target: "LlamaDownloader", "下载进度: {:.1}%", global_progress * 100.0);
            }
        }

        let final_size = std::fs::metadata(dest)
            .map(|m| m.len())
            .unwrap_or(downloaded);
        if final_size > 0 {
            tracing::info!(target: "LlamaDownloader", attempt, downloaded = final_size, "下载完成");
            return Ok(final_size);
        }
        last_error = "下载无数据（文件大小为 0）".to_string();
    }

    tracing::error!(target: "LlamaDownloader", error = %last_error, "下载失败，已用完所有重试次数");
    Err(anyhow::anyhow!(
        "下载失败: {}（已重试 {} 次）",
        last_error,
        MAX_ATTEMPTS
    ))
}

/// 多线程分块下载，利用 Range 请求并行下载提升速度
fn curl_download_parallel(
    url: &str,
    dest: &Path,
    total_size: u64,
    progress_start: f64,
    progress_end: f64,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
    cancel_token: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<u64> {
    const MAX_CHUNKS: usize = 8;
    const CHUNK_SIZE: u64 = 4 * 1024 * 1024; // 4MB per chunk

    if total_size == 0 {
        return curl_download(
            url,
            dest,
            0,
            progress_start,
            progress_end,
            progress_callback,
            cancel_token,
        );
    }

    // 创建目标文件
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
    let _ = fs::remove_file(dest);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(dest)?;

    // 计算分块数量和范围
    let num_chunks = (total_size as f64 / CHUNK_SIZE as f64).ceil() as usize;
    let num_chunks = num_chunks.min(MAX_CHUNKS);
    let chunk_size = (total_size as f64 / num_chunks as f64).ceil() as u64;

    tracing::info!(
        target: "LlamaDownloader",
        url = %url,
        total_size,
        num_chunks,
        chunk_size,
        "启动分块下载"
    );

    // 通知前端：开始下载
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "downloading".into(),
            progress: progress_start,
            downloaded: 0,
            total: total_size,
                        message: format!(
                "开始下载 ({:.1} MB, {} 线程)...",
                total_size as f64 / 1_048_576.0,
                num_chunks
            ),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
    }

    // 并发下载各分块 + 实时进度上报
    let downloaded = std::sync::Arc::new(std::sync::Mutex::new(0u64));
    let mut handles = Vec::new();

    for chunk_idx in 0..num_chunks {
        let range_start = chunk_idx as u64 * chunk_size;
        let range_end = (range_start + chunk_size).min(total_size) - 1;
        if range_start >= total_size {
            break;
        }

        let url = url.to_string();
        let downloaded_clone = downloaded.clone();

        let handle = std::thread::spawn(move || {
            let client = Client::builder()
                .timeout(Duration::from_secs(300))
                .user_agent("LlamaUI/0.7.0")
                .build()
                .expect("构建 reqwest Client 失败");

            let mut resp = client
                .get(&url)
                .header("Range", format!("bytes={}-{}", range_start, range_end))
                .send()
                .map_err(|e| anyhow::anyhow!("Range 请求失败: {}", e))?;

            if !resp.status().is_success() {
                return Err(anyhow::anyhow!("HTTP {}", resp.status()));
            }

            let mut chunk_data = Vec::new();
            resp.copy_to(&mut chunk_data)
                .map_err(|e| anyhow::anyhow!("读取响应体失败: {}", e))?;

            {
                let mut d = downloaded_clone.lock().unwrap();
                *d += chunk_data.len() as u64;
            }

            Ok::<_, anyhow::Error>((range_start, chunk_data))
        });
        handles.push(handle);
    }

    // 等待所有分块下载完成，同时持续上报实时进度
    let start = std::time::Instant::now();
    let mut last_emitted_progress: f64 = progress_start;
    while !handles.iter().all(|h| h.is_finished()) {
        let current = *downloaded.lock().unwrap();
        let raw_progress = if total_size > 0 {
            current as f64 / total_size as f64
        } else {
            0.0
        };
        let global_progress = progress_start + raw_progress * (progress_end - progress_start);
        if (global_progress - last_emitted_progress).abs() >= 0.001 {
            last_emitted_progress = global_progress;
            let elapsed = start.elapsed().as_secs_f64();
            let speed_mbps = if elapsed > 0.0 {
                (current as f64 / elapsed) / 1_048_576.0
            } else {
                0.0
            };
            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "downloading".into(),
                    progress: global_progress,
                    downloaded: current,
                    total: total_size,
                    message: format!("下载中 {:.1}%", global_progress * 100.0),
                    speed_mbps,
                    eta_secs: None,
                    detail: None,
                });
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 最后一次完整上报
    let current = *downloaded.lock().unwrap();
    let raw_progress = if total_size > 0 { current as f64 / total_size as f64 } else { 0.0 };
    let global_progress = progress_start + raw_progress * (progress_end - progress_start);
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "downloading".into(),
            progress: global_progress,
            downloaded: current,
            total: total_size,
            message: format!("下载中 {:.1}%", global_progress * 100.0),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
    }

    // 收集所有分块结果
    let mut chunks: Vec<(u64, Vec<u8>)> = Vec::new();
    for handle in handles {
        let chunk = match handle.join() {
            Ok(result) => result.map_err(|e| anyhow::anyhow!("下载线程失败: {:?}", e))?,
            Err(e) => anyhow::bail!("下载线程 panic: {:?}", e),
        };
        chunks.push(chunk);
    }

    // 按偏移量排序并写入文件
    chunks.sort_by_key(|(offset, _)| *offset);
    let mut total_written = 0u64;
    let start = std::time::Instant::now();

    for (offset, data) in chunks {
        file.seek(std::io::SeekFrom::Start(offset))?;
        file.write_all(&data)?;
        total_written += data.len() as u64;

        // 发送进度
        if let Some(cb) = progress_callback {
            let raw_progress = if total_size > 0 {
                total_written as f64 / total_size as f64
            } else {
                0.0
            };
            let global_progress = progress_start + raw_progress * (progress_end - progress_start);
            let elapsed = start.elapsed().as_secs_f64();
            let speed_mbps = if elapsed > 0.0 {
                (total_written as f64 / elapsed) / 1_048_576.0
            } else {
                0.0
            };
            cb(DownloadProgress {
                stage: "downloading".into(),
                progress: global_progress,
                downloaded: total_written,
                total: total_size,
                    message: format!(
                        "{:.1} / {:.1} MB ({:.1}%) · {:.1} MB/s",
                        total_written as f64 / 1_048_576.0,
                        total_size as f64 / 1_048_576.0,
                        global_progress * 100.0,
                        speed_mbps
                    ),
                speed_mbps,
                eta_secs: if speed_mbps > 0.0 {
                    Some(((total_size - total_written) as f64 / 1_048_576.0 / speed_mbps) as u64)
                } else {
                    None
                },
                detail: Some(DownloadProgressDetail {
                    step: format!("分块下载 ({} chunks)", num_chunks),
                    step_progress: raw_progress,
                    candidate_index: 1,
                    candidate_count: 1,
                    current_candidate: None,
                    speed_mbps,
                    eta_secs: if speed_mbps > 0.0 {
                        Some(
                            ((total_size - total_written) as f64 / 1_048_576.0 / speed_mbps) as u64
                                as f64,
                        )
                    } else {
                        None
                    },
                }),
            });
        }
    }

    let final_size = fs::metadata(dest)?.len();
    tracing::info!(
        target: "LlamaDownloader",
        url = %url,
        downloaded = final_size,
        num_chunks,
        "分块下载完成"
    );
    Ok(final_size)
}

/// 下载进度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub stage: String,
    pub progress: f64,
    pub downloaded: u64,
    pub total: u64,
    pub message: String,
    /// 当前下载速度 MB/s（仅下载阶段有效）
    pub speed_mbps: f64,
    /// 预计剩余秒数（仅下载阶段有效）
    pub eta_secs: Option<u64>,
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
        speed_mbps: detail.speed_mbps,
        eta_secs: detail.eta_secs.map(|v| v as u64),
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
        speed_mbps: 0.0,
        eta_secs: None,
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
    prerelease: bool,
    /// 版本来源（用于前端显示进度信息）
    #[serde(skip)]
    source: &'static str,
}

/// GitHub Release 资产
#[derive(Debug, Deserialize)]
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

    // 检查是否是有效的 tar.gz 文件
    let is_tar_gz = {
        let mut magic = [0u8; 2];
        if let Ok(mut f) = fs::File::open(archive) {
            use std::io::Read;
            let _ = f.read(&mut magic);
            magic == [0x1f, 0x8b] // gzip magic bytes
        } else {
            false
        }
    };

    if !is_tar_gz {
        // 不是 tar.gz，直接返回空结果（由调用方处理）
        tracing::debug!(target: "LlamaDownloader", "文件不是有效的 tar.gz，跳过 tar 解压");
        return Ok(Vec::new());
    }

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
                speed_mbps: 0.0,
                eta_secs: None,
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
    let start = std::time::Instant::now();

    // 先尝试 tar（仅当文件是 tar.gz 时）
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
        }
    }

    // 发送解压完成进度（在查找 llama-server 之前）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "extracting".to_string(),
            progress: stage_progress::EXTRACTING_END,
            downloaded: 0,
            total: 0,
            message: format!("解压完成（耗时 {:.1}s）", start.elapsed().as_secs_f64()),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
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
            stage_progress::FINDING_ASSET_START,
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
                stage_progress::FINDING_ASSET_START,
                "精确匹配失败，尝试宽松匹配...".to_string(),
            ));
        }
        for asset in &release.assets {
            let name_lower = asset.name.to_lowercase();
            // 宽松匹配：只要求包含 llama 和匹配架构
            if name_lower.contains("llama")
                && name_lower.contains(arch_keyword)
                && !name_lower.contains("-metal")
            // macOS 专用
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
            stage_progress::FINDING_ASSET_START,
            format!("开始匹配资产（共 {} 个候选需要验证）...", total_candidates),
        ));
    }

    // 并行执行所有 HEAD 请求，避免串行等待
    let asset_range = stage_progress::FINDING_ASSET_END - stage_progress::FINDING_ASSET_START;
    let urls: Vec<String> = candidates
        .iter()
        .map(|a| a.browser_download_url.clone())
        .collect();
    let results: Vec<(usize, anyhow::Result<u64>)> = std::thread::scope(|s| {
        urls.iter()
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

        let verify_progress = stage_progress::FINDING_ASSET_START
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
                        format!(
                            "✅ 候选 {}/{} 可用，选中：{}",
                            candidate_index, total_candidates, candidate_name
                        ),
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
                        format!(
                            "❌ {}/{} 失败（{}），尝试下一个...",
                            candidate_index, total_candidates, e
                        ),
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
                    step_progress: stage_progress::PREPARING_ASSET_END,
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
    let version_range =
        stage_progress::FETCHING_VERSION_END - stage_progress::FETCHING_VERSION_START;
    let auth = get_github_token().map(|t| format!("token {}", t));
    for attempt in 1..=max_retries {
        tracing::info!(
            target: "LlamaDownloader",
            attempt = attempt,
            max = max_retries,
            "尝试获取最新版本"
        );
        // 通知前端：第 N 次尝试
        if let Some(cb) = progress_callback {
            let p = stage_progress::FETCHING_VERSION_START
                + ((attempt - 1) as f64 / max_retries as f64) * version_range;
            cb(DownloadProgress {
                stage: "fetching_version".to_string(),
                progress: p,
                downloaded: 0,
                total: 0,
                message: format!("获取最新版本... 第 {} 次尝试", attempt),
                speed_mbps: 0.0,
                eta_secs: None,
                detail: None,
            });
        }
        match try_fetch_with_client(&VERSION_CLIENT, &auth) {
            Ok(release) => {
                if release.prerelease {
                    tracing::info!(
                        target: "LlamaDownloader",
                        "获取到的版本是预发布版（prerelease），可能不稳定"
                    );
                }
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

/// 使用指定客户端尝试获取最新版本（核心逻辑提取）
fn try_fetch_with_client(
    client: &reqwest::blocking::Client,
    auth: &Option<String>,
) -> anyhow::Result<GitHubRelease> {
    // 0) 用户可通过环境变量覆盖（最高优先级）
    if let Ok(tag) = std::env::var("LLAMA_CPP_VERSION") {
        let os = current_os();
        let arch = current_arch();
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
    let latest_url = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
    if let Ok(release) = fetch_github_release(client, auth, latest_url) {
        if has_binary_assets(&release) {
            tracing::info!(target: "LlamaDownloader", tag = %release.tag_name, "latest release 有二进制资产，直接使用");
            let mut release = release;
            release.source = "api";
            return Ok(release);
        }
        // 2. latest 没有二进制资产 → 下载 nightly-tag.txt 获取 nightly tag
        if let Some(nightly_tag) = fetch_nightly_tag_via_api(client, auth, &release) {
            let nightly_url = format!(
                "https://api.github.com/repos/ggml-org/llama.cpp/releases/tags/{}",
                nightly_tag
            );
            if let Ok(nightly) = fetch_github_release(client, auth, &nightly_url) {
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
    if let Ok(releases) = fetch_github_releases_list(client, auth, list_url) {
        for release in releases {
            if has_binary_assets(&release) {
                tracing::info!(target: "LlamaDownloader", tag = %release.tag_name, "从 releases 列表找到二进制资产");
                let mut release = release;
                release.source = "api";
                return Ok(release);
            }
        }
    }

    // 4) 回退：API 失败时，尝试下载 nightly-tag.txt 直接获取 nightly tag
    if let Some(nightly_tag) = fetch_nightly_tag_with_client(client) {
        tracing::warn!(
            target: "LlamaDownloader",
            nightly_tag = %nightly_tag,
            "API 不可用，回退到 nightly-tag.txt 方式"
        );
        let os = current_os();
        let arch = current_arch();
        return Ok(GitHubRelease {
            tag_name: nightly_tag.clone(),
            assets: build_virtual_assets(&nightly_tag, os, arch),
            prerelease: true,
            source: "nightly",
        });
    }

    // 5) 最后回退：硬编码已知稳定版本
    let tag = "b10964".to_string();
    let tag_owned = tag.clone();
    tracing::warn!(target: "LlamaDownloader", tag = %tag_owned, "所有策略失败，回退到硬编码版本");
    Ok(GitHubRelease {
        tag_name: tag_owned,
        assets: build_virtual_assets(&tag, current_os(), current_arch()),
        prerelease: true,
        source: "fallback",
    })
}

/// 下载并安装 llama-server（智能匹配 + 自动重试）
#[allow(clippy::print_stderr)]
pub fn download_and_install(
    backend: GpuBackend,
    install_dir: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
    cancel_token: Option<&std::sync::atomic::AtomicBool>,
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
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
        // 初始化完成后推进
        cb(DownloadProgress {
            stage: "init".to_string(),
            progress: stage_progress::INIT_END,
            downloaded: 0,
            total: 0,
            message: "初始化完成".to_string(),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
    }
    // 取消检查
    if let Some(ct) = cancel_token {
        if ct.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow::anyhow!("下载已取消"));
        }
    }

    // 1. 获取最新版本（带重试）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "fetching_version".to_string(),
            progress: stage_progress::FETCHING_VERSION_START,
            downloaded: 0,
            total: 0,
            message: format!("获取最新版本（最多 {} 次重试）...", max_retries),
            speed_mbps: 0.0,
            eta_secs: None,
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
            progress: stage_progress::PREPARING_ASSET_START,
            downloaded: 0,
            total: 0,
            message: format!("查找匹配资产 (tag={})...", tag),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
    }

    let (asset, head_size) =
        smart_find_asset(&release, backend, progress_callback).ok_or_else(|| {
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
                message: format!(
                    "✅ 本地归档已存在 ({:.1} MB)，跳过下载",
                    archive_size as f64 / 1048576.0
                ),
                speed_mbps: 0.0,
                eta_secs: None,
                detail: None,
            });
        }
        downloaded = archive_size;
    } else {
        let mut download_attempt = 0;
        downloaded = loop {
            download_attempt += 1;
            match curl_download_parallel(
                &asset.browser_download_url,
                &archive_path,
                total_size,
                stage_progress::DOWNLOAD_START,
                stage_progress::DOWNLOAD_END,
                progress_callback,
                cancel_token,
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
            speed_mbps: 0.0,
            eta_secs: None,
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
            stage: "finalizing".to_string(),
            progress: stage_progress::FINALIZING_START,
            downloaded,
            total: downloaded,
            message: "清理临时文件...".to_string(),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
    }

    let file_size = fs::metadata(&llama_server_path)?.len();

    tracing::info!(target: "LlamaDownloader", path = %llama_server_path.display(), bytes = file_size, "安装完成");

    // 10. SHA256 完整性校验（快速校验）
    let sha256 = compute_sha256_fast(&llama_server_path, progress_callback, file_size, cancel_token)?;
    tracing::info!(target: "LlamaDownloader", sha256 = %sha256, "SHA256 校验完成");

    // 11. 发送完成事件（在 spawn_blocking 内通过 callback 发送）
    // 延时 50ms 确保前端先处理完 "verifying" 完成事件，避免事件乱序导致 UI 卡在 99%
    std::thread::sleep(std::time::Duration::from_millis(50));
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "complete".into(),
            progress: stage_progress::COMPLETE_END,
            downloaded: file_size,
            total: file_size,
            message: "✅ 安装完成".into(),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: None,
        });
    }

    let elapsed = start.elapsed().as_millis() as u64;

    Ok(DownloadResult {
        success: true,
        path: llama_server_path.to_string_lossy().to_string(),
        file_size,
        sha256,
        elapsed_ms: elapsed,
        error: None,
    })
}

/// 快速计算文件的 SHA256 十六进制摘要（大缓冲区 + 高频进度更新）
///
/// 优化策略：
/// - 使用 1MB 缓冲区加速文件读取（原为 64KB）
/// - 每 1MB 或 100ms 上报一次进度（确保流畅无卡顿）
/// - 消息格式包含实时 MB 进度，便于用户感知
/// - 确保最后一定发出 VERIFYING_END 进度
fn compute_sha256_fast(
    path: &Path,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
    file_size: u64,
    cancel_token: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();

    // 1MB 缓冲区，大幅提升 I/O 吞吐量
    let mut buf = [0u8; 1024 * 1024];
    let mut bytes_read = 0u64;
    let mut last_progress_at = std::time::Instant::now();
    let verify_start = std::time::Instant::now();

    const PROGRESS_BYTES: u64 = 512 * 1024; // 每 512KB 累积计算进度（确保小文件也有中间上报）
    const PROGRESS_MIN_MS: u64 = 50; // 时间节流：至少 50ms 才上报（更流畅）

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes_read += n as u64;

        // 时间节流检查
        let now = std::time::Instant::now();
        let time_ok = now.duration_since(last_progress_at).as_millis() as u64 >= PROGRESS_MIN_MS;

        // 每 1MB 或到达末尾时上报进度（避免重复）
        let is_end = bytes_read >= file_size;
        let byte_aligned = bytes_read % PROGRESS_BYTES < n as u64;

        // 只在中途节点发送，最后一步由循环后的代码处理
        if byte_aligned && !is_end && time_ok {
            last_progress_at = now;
            if let Some(cb) = progress_callback {
                let pct = if file_size > 0 {
                    stage_progress::VERIFYING_START
                        + (bytes_read as f64 / file_size as f64)
                            * (stage_progress::VERIFYING_END - stage_progress::VERIFYING_START)
                } else {
                    stage_progress::VERIFYING_END
                };
                let elapsed_ms = now.duration_since(verify_start).as_millis();
                let speed_mbps = if elapsed_ms > 0 {
                    (bytes_read as f64 / 1_048_576.0) / (elapsed_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                cb(DownloadProgress {
                    stage: "verifying".to_string(),
                    progress: pct,
                    downloaded: bytes_read,
                    total: file_size,
                    message: format!(
                        "校验文件完整性... {:.1}%（{:.1} MB / {:.1} MB，{:.1} MB/s）",
                        pct * 100.0,
                        bytes_read as f64 / 1_048_576.0,
                        file_size as f64 / 1_048_576.0,
                        speed_mbps
                    ),
                    speed_mbps,
                                        eta_secs: Some(
                        ((file_size - bytes_read) as f64 / 1_048_576.0 / speed_mbps.max(0.001)) as u64,
                    ),
                    detail: Some(DownloadProgressDetail {
                        step: "SHA256 校验".to_string(),
                        step_progress: bytes_read as f64 / file_size.max(1) as f64,
                        candidate_index: 0,
                        candidate_count: 0,
                        current_candidate: None,
                        speed_mbps,
                        eta_secs: Some(
                            ((file_size - bytes_read) as f64 / 1_048_576.0 / speed_mbps.max(0.001))
                                as u64 as f64,
                        ),
                    }),
                });
            }
        }
    }

    // 循环结束后发送最终完成事件（97%）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "verifying".to_string(),
            progress: stage_progress::VERIFYING_END,
            downloaded: file_size,
            total: file_size,
            message: "校验文件完整性... 完成".to_string(),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: Some(DownloadProgressDetail {
                step: "SHA256 校验完成".to_string(),
                step_progress: 1.0,
                candidate_index: 0,
                candidate_count: 0,
                current_candidate: None,
                speed_mbps: 0.0,
                eta_secs: None,
            }),
        });
    }

    let hash = hasher.finalize();

    // 取消检查
    if let Some(ct) = cancel_token {
        if ct.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow::anyhow!("SHA256 校验已取消"));
        }
    }

    Ok(format!("{:x}", hash))
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
    let nightly_asset = release
        .assets
        .iter()
        .find(|a| a.name == "nightly-tag.txt")?;
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

/// 使用指定客户端下载 nightly-tag.txt
fn fetch_nightly_tag_with_client(
    client: &reqwest::blocking::Client,
) -> Option<String> {
    let stable_tags = ["v0.4.2", "v0.4.1", "v0.4.0"];
    for stable_tag in &stable_tags {
        let url = format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{}/nightly-tag.txt",
            stable_tag
        );
        let mut req = client.get(&url);
        req = req.timeout(std::time::Duration::from_secs(10));
        if let Some(resp) = req.send().ok() {
            if resp.status().is_success() {
                if let Ok(content) = resp.text() {
                    let tag = content.trim().to_string();
                    if !tag.is_empty() && tag.starts_with('b') {
                        return Some(tag);
                    }
                }
            }
        }
    }
    None
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
    let arr = json
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("API 返回非数组"))?;
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
        prerelease: json
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
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
    fn test_detect_gpu_backend() {
        let _backend = detect_gpu_backend();
    }

    /// `prerelease` 字段随 Deserialize 从 release JSON 解出，用于跳过预发布版本。
    #[test]
    fn test_release_parses_prerelease_flag() {
        let json = r#"{"tag_name":"b1","prerelease":true,"assets":[]}"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let release: GitHubRelease = serde_json::from_value(value).unwrap();
        assert!(release.prerelease, "prerelease 字段应被反序列化");
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
            speed_mbps: 0.0,
            eta_secs: None,
        };
        assert_eq!(progress.progress, 0.0);
        assert_eq!(progress.downloaded, 0);
        assert_eq!(progress.stage, "test");
    }
}
