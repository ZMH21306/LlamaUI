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
use std::sync::OnceLock;
use std::time::Duration;

/// 共享的阻塞 HTTP 客户端（直连 reqwest，不再依赖自研下载引擎）。
///
/// 关键优化：所有 chunk 线程共享同一个 Client 实例，而不是每次重试都
/// 新建 `Client::builder().build()`。每个新 Client 都有独立的连接池
/// 和 TLS 会话缓存，32 chunks × 3 retries = 96 次重建会导致：
/// - 96 次独立 TLS 握手，会话完全无法复用
/// - 连接池每次被丢弃，已建立的 TCP 连接全部作废
/// - 实测吞吐只有实际带宽的 1/10 左右
///
/// 另外开启 `tcp_nodelay(true)` 关闭 Nagle 算法，
/// 避免「小包 + 延迟 ACK」交互引入 20~40ms 的额外往返延迟。
#[allow(clippy::expect_used)]
static SHARED_CLIENT: OnceLock<Client> = OnceLock::new();

/// 获取共享阻塞 HTTP 客户端。
///
/// 连接池容量必须 >= 最大并发分块数，否则多出的分块会反复新建 TCP+TLS
/// 连接（每次约 3~5 个 RTT），在丢包链路上会放大成"越并发越慢"。
/// 同时把空闲连接保留时间拉长，避免并发分块之间频繁重连。
fn shared_client() -> &'static Client {
    SHARED_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(128)
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .user_agent("LlamaUI/0.7.0")
            .build()
            .expect("构建 reqwest blocking Client 失败")
    })
}

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
/// 用 reqwest 发送 HEAD 请求验证 URL 可用性（带 TLS 证书验证），并返回 Content-Length 大小
///
/// 复用 `shared_client()` 而不是每次 `Client::builder().build()`：
/// 资产匹配阶段最多会串行探测 4 个候选 URL，每次新建 Client 都会
/// 丢弃连接池并重新做一次 TLS 握手，叠加在「起步阶段」上很可观。
/// 注意：`connect_timeout` 只能在 `ClientBuilder` 上设置（请求级 API 不提供），
/// 因此连接超时沿用 `shared_client()` 的 30s，这里只覆盖整体超时。
fn curl_head(url: &str) -> anyhow::Result<u64> {
    let response = shared_client()
        .head(url)
        .timeout(Duration::from_secs(15))
        .send()?;
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
/// - ① 初始化              : 0%  ~ 1%   （1%）
/// - ② 获取最新版本         : 1%  ~ 3%   （2%）
/// - ③ 准备匹配资产         : 3%  ~ 4%   （1%）
/// - ④ 匹配/验证资产       : 4%  ~ 8%   （4%，并行HEAD验证）
/// - ⑤ 下载安装包           : 8%  ~ 90%  （82%，主阶段，权重最大）
/// - ⑥ 解压归档             : 90% ~ 94%  （4%）
/// - ⑦ SHA256 校验          : 94% ~ 98%  （4%）
/// - ⑧ 清理收尾             : 98% ~ 99%  （1%）
/// - ⑨ 完成                 : 99% ~ 100% （1%）
///
/// 设计原则：用户等待时间几乎全部消耗在真实下载上，因此把进度条
/// 的绝大部分宽度分配给「下载安装包」阶段。原先的分配里前置阶段
/// （版本获取 + 资产匹配）占 22% 宽度，但实际耗时可能占总时间的一半，
/// 导致进度条长时间停在 20% 之前，视觉上像"卡住"。
pub mod stage_progress {
    pub const INIT_END: f64 = 0.01;
    pub const FETCHING_VERSION_START: f64 = 0.01;
    pub const PREPARING_ASSET_START: f64 = 0.03;
    pub const PREPARING_ASSET_END: f64 = 0.04;
    pub const FINDING_ASSET_START: f64 = 0.04;
    pub const FINDING_ASSET_END: f64 = 0.08;
    pub const DOWNLOAD_START: f64 = 0.08;
    pub const DOWNLOAD_END: f64 = 0.90;
    pub const EXTRACTING_END: f64 = 0.94;
    pub const VERIFYING_START: f64 = 0.94;
    pub const VERIFYING_END: f64 = 0.98;
    pub const FINALIZING_START: f64 = 0.98;
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
            let backoff =
                Duration::from_millis(500 * u64::from(attempt)).min(Duration::from_secs(2));
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

        let mut resp = match shared_client().get(url).send() {
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

            // 时间节流：每 250ms 至少上报一次，或下载完成时上报。
            // 注意：必须以时间为准做「或」判断。原实现用
            // `boundary_cross && time_ok`，其中 boundary_cross 依赖
            // `downloaded % 100_000` 恰好跨整数边界，命中率不稳定，
            // 在高速下载时会出现 4~6s 才回调一次、UI 卡顿的问题。
            let now = std::time::Instant::now();
            let elapsed_ms = now.duration_since(last_progress_at).as_millis() as u64;
            let should_emit = elapsed_ms >= 250 || downloaded == size;
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
    // 分块策略与线程数不再硬编码：由下方自适应算法按文件大小推导。
    //
    // 旧实现的两个问题：
    // 1) `CHUNK_SIZE = 512KB` 仅用于估算块数，随后被 `min(32)` 截断，
    //    再按 `total/32` 反推实际块大小 —— 注释说的 512KB 根本不生效，
    //    250MB 文件实际切成 32 块 × 7.8MB。在低速链路上单块远超读超时，
    //    必然整块失败重下（本次优化要修的主因）。
    // 2) 块数固定 32，小文件白白起 32 个线程，大文件并发又不够。

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

    // 创建目标文件（支持断点续传：重试时不截断，跳过已下载的分块）
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(dest)?;

    // 自适应分块：目标每块约 2MiB，块数按文件大小缩放，并封顶 96 块
    // （既保证大文件并发足够，又不让线程数随体积无限膨胀）。
    const TARGET_CHUNK_SIZE: u64 = 2 * 1024 * 1024; // 2 MiB
    const MIN_CHUNK_SIZE: u64 = 256 * 1024; // 256 KiB
    const MAX_CHUNKS: usize = 96;
    let mut num_chunks =
        ((total_size + TARGET_CHUNK_SIZE - 1) / TARGET_CHUNK_SIZE).max(1) as usize;
    num_chunks = num_chunks.min(MAX_CHUNKS);
    let mut chunk_size = (total_size + num_chunks as u64 - 1) / num_chunks as u64;
    // 极小文件（< 256KiB）直接单块处理，避免起线程做无意义的分块
    if total_size > 0 && chunk_size < MIN_CHUNK_SIZE {
        num_chunks = 1;
        chunk_size = total_size;
    }

    // 读取已下载的分块，用于断点续传。
    //
    // 只记录每块的「已有字节数」，不把文件内容读进内存：
    // 原实现是 `vec![Vec<u8>>`，会把整个文件（可达 250MB+）一次性读入
    // 内存，仅为了判断 `is_empty()`，白白吃掉数百 MB 内存和一次全量磁盘读。
    let mut existing_bytes: Vec<u64> = vec![0u64; num_chunks];
    // 文件当前物理长度（稀疏写入时的高水位）。
    // 不能仅用 `file_len > end` 判定整块完成——稀疏文件在 `end` 之后
    // 可能仍是空洞，长度够并不代表本块区间已真正写满。
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    for i in 0..num_chunks {
        let start = i as u64 * chunk_size;
        let end = (start + chunk_size).min(total_size) - 1;
        let len = end - start + 1;
        // 起点已超出当前文件长度：本块完全无数据
        if start >= file_len {
            continue;
        }
        // 读满整块做校验：只有真正逐字节读完才认为该块完整。
        // 旧实现只探 1 字节 + 比对总长度，稀疏文件下会误判为已完成，
        // 导致下载出中间带空洞的损坏文件。
        if file.seek(std::io::SeekFrom::Start(start)).is_ok() {
            let mut remaining = (file_len - start).min(len);
            let mut buf = vec![0u8; 64 * 1024];
            let mut got: u64 = 0;
            let mut ok = true;
            while remaining > 0 {
                let want = remaining.min(buf.len() as u64) as usize;
                match file.read(&mut buf[..want]) {
                    Ok(0) => {
                        // 提前 EOF：数据不完整
                        ok = false;
                        break;
                    }
                    Ok(n) => {
                        got += n as u64;
                        remaining -= n as u64;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && got == len {
                existing_bytes[i] = len;
            }
        }
    }

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

    // 并发下载各分块 + 实时进度上报。
    //
    // 关键设计：每个分块独立设置较短超时 + 逐块重试。
    // 本机到 GitHub CDN 带宽很低且极不稳定（实测 30~160KB/s，
    // 大块 Range 请求经常 0 字节超时），若按「整块失败才重试」的
    // 旧逻辑，32MB/块必然整体失败。改成 512KB/块后，单块在
    // 60s 内即使只跑 100KB/s 也能下完；失败时只重试该小块，
    // 已完成的分块不会浪费。
    // 用无锁原子计数器累加全局进度。
    // 96 个分块线程每读一个缓冲就要更新一次进度；若用 Mutex，
    // 锁竞争 + 唤醒开销会成为主要瓶颈，实测吞吐掉到实际带宽的零头。
    let downloaded = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let failed_offsets = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for chunk_idx in 0..num_chunks {
        let range_start = chunk_idx as u64 * chunk_size;
        let range_end = (range_start + chunk_size).min(total_size) - 1;
        if range_start >= total_size {
            break;
        }

        // 断点续传：已存在完整数据的分块直接计入进度，不再重新下载
        if existing_bytes[chunk_idx] > 0 {
            // 原子累加，无需加锁
            downloaded.fetch_add(
                existing_bytes[chunk_idx],
                std::sync::atomic::Ordering::Relaxed,
            );
            continue;
        }

        let url = url.to_string();
        let dest_path = dest.to_path_buf();
        let downloaded_clone = downloaded.clone();
        let failed_offsets_clone = failed_offsets.clone();

        let handle = std::thread::spawn(move || {
            const MAX_CHUNK_RETRIES: u32 = 3;
            // 超时随块大小缩放，不再用固定 60s：
            // 2MiB 块在 30KB/s 的链路上需要 ~70s，固定 60s 会让每一块
            // 都在接近完成时被判超时，然后整块作废重下 —— 这正是当前
            // "越下载越慢"的直接原因。
            // 按 32KiB/s 的悲观速率预留 4 倍余量，再夹在 [60s, 600s]。
            let chunk_timeout = Duration::from_secs(
                ((range_end - range_start + 1) / 32 * 4).clamp(60, 600),
            );
            // 读缓冲提升到 256KB：
            // 64KB 在百兆以上链路上会让 read() 系统调用过于频繁，
            // 单次调用摊销开销占比可观。256KB 是吞吐/内存的平衡点
            // （96 线程 × 256KB = 24MB 常驻，远低于旧实现把整个文件读进内存的峰值）。
            const STREAM_BUF: usize = 256 * 1024;

            let mut last_err: Option<anyhow::Error> = None;
            for attempt in 1..=MAX_CHUNK_RETRIES {
                // 重试退避：第 2、3 次分别等待 200ms / 400ms，
                // 避免瞬时网络抖动时立刻重连形成忙循环。
                if attempt > 1 {
                    std::thread::sleep(Duration::from_millis(200 * u64::from(attempt - 1)));
                    tracing::debug!(
                        target: "LlamaDownloader",
                        range_start,
                        attempt,
                        "分块重试"
                    );
                }
                // 所有 chunk 共享同一个 Client 实例（带 tcp_nodelay + 连接池复用），
                // 避免每次重试都新建 Client 导致 96 次独立 TLS 握手。
                let client = shared_client();

                let mut resp = match client
                    .get(&url)
                    .header("Range", format!("bytes={}-{}", range_start, range_end))
                    .timeout(chunk_timeout)
                    .send()
                {
                    Ok(r) => r,
                    Err(e) => {
                        last_err = Some(anyhow::anyhow!("Range 请求失败: {}", e));
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    last_err = Some(anyhow::anyhow!("HTTP {}", resp.status()));
                    continue;
                }

                // 流式读取 → 直接写入文件对应偏移。
                // 原来用 `resp.copy_to(&mut chunk_data)` 把整个 chunk 先读进
                // 512KB Vec，等所有分块完成后再统一 write_all 落盘，
                // 造成「网络读取」和「磁盘写入」两段完全串行，且 32 个 Vec
                // 同时驻留内存（峰值 = 整个文件大小）。
                //
                // 现在每个 chunk 线程持有独立 File 句柄，网络数据一到就立刻
                // 顺序写入对应偏移：下载与落盘流水线并行，内存恒定 64KB。
                let mut file = match fs::OpenOptions::new().write(true).open(&dest_path) {
                    Ok(f) => f,
                    Err(e) => {
                        last_err = Some(anyhow::anyhow!("打开输出文件失败: {}", e));
                        continue;
                    }
                };
                if let Err(e) = file.seek(std::io::SeekFrom::Start(range_start)) {
                    last_err = Some(anyhow::anyhow!("定位写入位置失败: {}", e));
                    continue;
                }

                let mut buf = vec![0u8; STREAM_BUF];
                let mut local_downloaded: u64 = 0;
                let mut read_ok = true;
                loop {
                    let n = match resp.read(&mut buf) {
                        Ok(n) => n,
                        Err(e) => {
                            last_err = Some(anyhow::anyhow!("读取响应体失败: {}", e));
                            read_ok = false;
                            break;
                        }
                    };
                    if n == 0 {
                        break;
                    }
                    if let Err(e) = file.write_all(&buf[..n]) {
                        last_err = Some(anyhow::anyhow!("写入文件失败: {}", e));
                        read_ok = false;
                        break;
                    }
                    // 原子累加进度（无锁）。旧实现每 64KB 抢一次 Mutex，
                    // 高并发下锁竞争直接把吞吐压到实际带宽的零头。
                    downloaded_clone.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                    local_downloaded += n as u64;
                }

                if read_ok {
                    return Ok::<_, anyhow::Error>((range_start, local_downloaded));
                }
            }

            // 该分块重试耗尽，记录失败偏移以便上层重试
            if let Some(e) = &last_err {
                tracing::warn!(
                    target: "LlamaDownloader",
                    range_start,
                    range_end,
                    error = %e,
                    "分块下载失败，已重试 {} 次",
                    MAX_CHUNK_RETRIES
                );
                failed_offsets_clone.lock().unwrap().push(range_start);
            }
            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("分块下载未知失败")))
        });
        handles.push(handle);
    }

    // 等待所有分块下载完成，同时持续上报实时进度。
    //
    // 关键：分块可能因超时/失败而中断，必须在等待循环内**重试失败分块**，
    // 否则 `handles` 永远不会全部 finished，下载会无限卡住。
    let start = std::time::Instant::now();
    let mut last_emitted_progress: f64 = progress_start;
    // 强制上报间隔：即使进度差不足 0.0005，也保证 UI 至少每 500ms 收到一次事件。
    // 原实现只按 `进度差 >= 0.0005` 触发，在高速下载时两次回调可间隔数秒。
    const CHUNK_EMIT_MIN_INTERVAL: std::time::Duration = Duration::from_millis(500);
    let mut last_emit_at = std::time::Instant::now();
    let mut retry_round = 0u32;
    const MAX_RETRY_ROUNDS: u32 = 5;

    loop {
        let all_done = handles.iter().all(|h| h.is_finished());
        if all_done {
            break;
        }

        let current = downloaded.load(std::sync::atomic::Ordering::Relaxed);
        let raw_progress = if total_size > 0 {
            current as f64 / total_size as f64
        } else {
            0.0
        };
        let global_progress = progress_start + raw_progress * (progress_end - progress_start);
        let time_due = last_emit_at.elapsed() >= CHUNK_EMIT_MIN_INTERVAL;
        if (global_progress - last_emitted_progress).abs() >= 0.0005 || time_due {
            last_emitted_progress = global_progress;
            last_emit_at = std::time::Instant::now();
            let elapsed = start.elapsed().as_secs_f64();
            let speed_mbps = if elapsed > 0.0 {
                (current as f64 / elapsed) / 1_048_576.0
            } else {
                0.0
            };
            let eta_secs = if speed_mbps > 0.0 {
                Some(((total_size - current) as f64 / 1_048_576.0 / speed_mbps) as u64)
            } else {
                None
            };
            if let Some(cb) = progress_callback {
                cb(DownloadProgress {
                    stage: "downloading".into(),
                    progress: global_progress,
                    downloaded: current,
                    total: total_size,
                    message: format!(
                        "下载中 {:.1}% · {:.1}/{:.1} MB · {:.2} MB/s · {}",
                        global_progress * 100.0,
                        current as f64 / 1_048_576.0,
                        total_size as f64 / 1_048_576.0,
                        speed_mbps,
                        match eta_secs {
                            Some(s) => format!("约还需 {} 秒", s),
                            None => "计算中...".to_string(),
                        }
                    ),
                    speed_mbps,
                    eta_secs,
                    detail: Some(DownloadProgressDetail {
                        step: format!("分块下载 ({} chunks)", num_chunks),
                        step_progress: raw_progress,
                        candidate_index: 1,
                        candidate_count: 1,
                        current_candidate: None,
                        speed_mbps,
                        eta_secs: eta_secs.map(|v| v as f64),
                    }),
                });
            }
        }

        // 检测失败分块并重试
        let failed = std::mem::take(&mut *failed_offsets.lock().unwrap());
        if !failed.is_empty() && retry_round < MAX_RETRY_ROUNDS {
            retry_round += 1;
            for offset in failed {
                let end = (offset + chunk_size).min(total_size) - 1;
                let url = url.to_string();
                let dest_path = dest.to_path_buf();
                let downloaded_clone = downloaded.clone();
                let failed_offsets_clone = failed_offsets.clone();
                handles.push(std::thread::spawn(move || {
                    const MAX_CHUNK_RETRIES: u32 = 3;
                    // 与首轮一致：超时按块大小缩放，缓冲同为 256KB
                    let chunk_timeout = Duration::from_secs(
                        ((end - offset + 1) / 32 * 4).clamp(60, 600),
                    );
                    const STREAM_BUF: usize = 256 * 1024;
                    let mut last_err: Option<anyhow::Error> = None;
                    for attempt in 1..=MAX_CHUNK_RETRIES {
                        // 重试退避，与首轮保持一致
                        if attempt > 1 {
                            std::thread::sleep(Duration::from_millis(200 * u64::from(attempt - 1)));
                            tracing::debug!(
                                target: "LlamaDownloader",
                                offset,
                                attempt,
                                "重试分块"
                            );
                        }
                        // 同样复用共享 Client（连接池 + TLS 会话 + tcp_nodelay）
                        let client = shared_client();

                        let mut resp = match client
                            .get(&url)
                            .header("Range", format!("bytes={}-{}", offset, end))
                            .timeout(chunk_timeout)
                            .send()
                        {
                            Ok(r) => r,
                            Err(e) => {
                                last_err = Some(anyhow::anyhow!("Range 请求失败: {}", e));
                                continue;
                            }
                        };
                        if resp.status().as_u16() != 206 {
                        // 服务器忽略 Range 头返回 200/其他状态码，会导致分块互相覆盖、文件损坏。
                        // 记录错误并进入下一次重试，避免静默损坏。
                        let status = resp.status();
                        let cr = resp.headers().get("Content-Range").cloned();
                        last_err = Some(anyhow::anyhow!(
                            "期望 206 Partial Content，实际 {}，Content-Range: {:?}",
                            status,
                            cr
                        ));
                        continue;
                    }
                    // 验证 Content-Range 与请求一致，防止透明代理/边缘节点返回错误区间
                    if let Some(cr) = resp.headers().get("Content-Range") {
                        let cr_str = cr.to_str().unwrap_or("");
                        // 格式: "bytes start-end/total"
                        if !cr_str.starts_with("bytes ") || !cr_str.contains(&format!("{}-{}", offset, end)) {
                            last_err = Some(anyhow::anyhow!(
                                "Content-Range 不匹配：期望 bytes {}-{}，实际 {}",
                                offset, end, cr_str
                            ));
                            continue;
                        }
                    }
                    let expected_len = end - offset + 1;

                        // 与首轮一致：流式读取并直接写入文件偏移
                        let mut file = match fs::OpenOptions::new().write(true).open(&dest_path) {
                            Ok(f) => f,
                            Err(e) => {
                                last_err = Some(anyhow::anyhow!("打开输出文件失败: {}", e));
                                continue;
                            }
                        };
                        if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)) {
                            last_err = Some(anyhow::anyhow!("定位写入位置失败: {}", e));
                            continue;
                        }

                        let mut buf = vec![0u8; STREAM_BUF];
                        let mut local_downloaded: u64 = 0;
                        loop {
                            let n = match resp.read(&mut buf) {
                                Ok(n) => n,
                                Err(e) => {
                                    last_err = Some(anyhow::anyhow!("读取响应体失败: {}", e));
                                    break;
                                }
                            };
                            if n == 0 {
                                break;
                            }
                            if let Err(e) = file.write_all(&buf[..n]) {
                                last_err = Some(anyhow::anyhow!("写入文件失败: {}", e));
                                break;
                            }
                            downloaded_clone.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                            local_downloaded += n as u64;
                        }

                        // 必须收齐预期字节数，否则视为不完整
                        if local_downloaded != expected_len && last_err.is_none() {
                            last_err = Some(anyhow::anyhow!(
                                "分块不完整：期望 {} 字节，实际 {} 字节",
                                expected_len,
                                local_downloaded
                            ));
                        }
                        if last_err.is_none() {
                            return Ok::<_, anyhow::Error>((offset, local_downloaded));
                        }
                        // 进度回退：本次已计入 downloaded 的字节要扣回去，
                        // 否则失败重试会让全局进度虚高，甚至超过 total_size。
                        downloaded_clone
                            .fetch_sub(local_downloaded, std::sync::atomic::Ordering::Relaxed);
                    }
                    if let Some(e) = &last_err {
                        tracing::warn!(
                            target: "LlamaDownloader",
                            offset,
                            error = %e,
                            "重试分块仍失败，标记为失败"
                        );
                        match failed_offsets_clone.lock() {
                            Ok(mut guard) => {
                                guard.push(offset);
                            }
                            Err(poisoned) => {
                                poisoned.into_inner().push(offset);
                            }
                        }
                    }
                    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("分块下载未知失败")))
                }));
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // 最后一次完整上报
    let current = downloaded.load(std::sync::atomic::Ordering::Relaxed);
    let raw_progress = if total_size > 0 {
        current as f64 / total_size as f64
    } else {
        0.0
    };
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

    // 收集所有分块结果。
    //
    // 关键：不能用 `?` 立刻 bail——那会丢弃已成功下载的分块，
    // 导致弱网下每次重试都从零开始。各 chunk 线程已把数据流式写入
    // 文件对应偏移，这里只需汇总成功/失败数量即可。
    let mut ok_chunks = 0usize;
    let mut chunk_errors: Vec<String> = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok(_)) => ok_chunks += 1,
            Ok(Err(e)) => chunk_errors.push(format!("{:?}", e)),
            Err(_) => chunk_errors.push("下载线程 panic".to_string()),
        }
    }

    if !chunk_errors.is_empty() {
        // 数据已由各线程流式落盘，这里只需 flush 落盘保证续传可用
        file.flush()?;
        tracing::warn!(
            target: "LlamaDownloader",
            ok_chunks,
            failed = chunk_errors.len(),
            "部分分块失败，保留已下载数据供续传"
        );
        return Err(anyhow::anyhow!(
            "{}/{} 个分块下载失败：{}",
            chunk_errors.len(),
            num_chunks,
            chunk_errors
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    // 全部成功：确保缓冲区落盘（数据已由 chunk 线程写入正确偏移）
    file.flush()?;
    tracing::info!(
        target: "LlamaDownloader",
        url = %url,
        total_size,
        num_chunks,
        ok_chunks,
        "分块下载完成"
    );
    Ok(total_size)
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
    pub version: String, // 添加版本字段
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
    /// 从字符串解析 GPU 后端
    pub fn parse_backend(s: &str) -> Self {
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
    /// 资产列表直接来自 GitHub API，名称/URL/size 均已确认真实，
    /// 无需再做 HEAD 探测（HEAD 仅用于「猜测名称」的直连兜底策略）。
    #[serde(skip)]
    trusted_assets: bool,
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
    // 注意：llama.cpp 资产命名里的 CUDA 小版本会持续演进（12.4 / 13.3 / 13.4 ...），
    // 因此对 CUDA 采用「主版本族」匹配（cuda-12 / cuda-13），避免绑定到某个已过期的精确小版本。
    let backend_keywords: Vec<&str> = match backend {
        GpuBackend::Cuda12_4 => vec!["cuda-12.4", "cuda-12", "cuda"],
        GpuBackend::Cuda13_3 => vec!["cuda-13.4", "cuda-13.3", "cuda-13", "cuda"],
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
                // 去重：多个关键词可能命中同一资产
                if candidates.iter().any(|c| std::ptr::eq(*c, asset)) {
                    continue;
                }
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

    // 优先选择「不含 CUDA 运行时打包」的普通 llama- 包：
    // `cudart-llama-*.zip` 会额外捆绑 CUDA runtime（体积可达 400MB+），
    // 而本机已装有 CUDA 运行时，无需重复下载。
    candidates.sort_by_key(|a| {
        let n = a.name.to_lowercase();
        if n.starts_with("cudart-") { 1 } else { 0 }
    });

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

    // 资产来自 GitHub API：名称、URL、size 均已确认真实，直接选中，**无需 HEAD 探测**。
    // 这避免了「API 已验证 → 再逐个 HEAD 复查」的重复等待。
    if release.trusted_assets {
        if let Some(asset) = candidates.first().copied() {
            tracing::info!(
                target: "LlamaDownloader",
                name = %asset.name,
                size = asset.size,
                "✅ 资产列表来自 GitHub API，跳过 HEAD 探测直接选用"
            );
            if let Some(cb) = progress_callback {
                cb(progress_simple(
                    "finding_asset",
                    stage_progress::FINDING_ASSET_END,
                    format!("✅ 已匹配：{}", asset.name),
                ));
            }
            return Some((asset, asset.size));
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

    // 并行执行所有 HEAD 请求，但使用通道确保第一个成功的即刻返回
    let asset_range = stage_progress::FINDING_ASSET_END - stage_progress::FINDING_ASSET_START;
    let (tx, rx) = std::sync::mpsc::channel();
    let urls: Vec<String> = candidates.iter().map(|a| a.browser_download_url.clone()).collect();

    // 启动所有验证线程
    for (i, url) in urls.iter().enumerate() {
        let tx = tx.clone();
        let url_owned = url.clone();
        std::thread::spawn(move || {
            let result = curl_head(&url_owned);
            if tx.send((i, result)).is_ok() {}
        });
    }

    // 立即检查通道，第一个成功的结果就返回
    while let Ok((i, result)) = rx.recv() {
        let candidate = &candidates[i];
        let candidate_index = (i + 1) as u32;
        let candidate_name = &candidate.name;

        match result {
            Ok(content_length) => {
                tracing::info!(
                    target: "LlamaDownloader",
                    name = %candidate_name,
                    url = %candidate.browser_download_url,
                    "✅ URL 可用，选择此资产"
                );

                // 通知前端：验证成功
                if let Some(cb) = progress_callback {
                    cb(progress_with(
                        "finding_asset",
                        stage_progress::FINDING_ASSET_END,
                        u64::from(candidate_index),
                        u64::from(total_candidates),
                        format!(
                            "✅ 候选 {}/{} 可用，选中：{}",
                            candidate_index, total_candidates, candidate_name
                        ),
                        DownloadProgressDetail {
                            step: format!("✅ 选中：{}", candidate_name),
                            step_progress: stage_progress::FINDING_ASSET_END,
                            candidate_index,
                            candidate_count: total_candidates,
                            current_candidate: Some(candidate_name.clone()),
                            speed_mbps: 0.0,
                            eta_secs: None,
                        },
                    ));
                }
                return Some((candidate, content_length));
            }
            Err(e) => {
                tracing::debug!(
                    target: "LlamaDownloader",
                    url = %candidate.browser_download_url,
                    error = %e,
                    "❌ URL 不可用，尝试下一个"
                );

                // 通知前端：验证失败
                if let Some(cb) = progress_callback {
                    cb(progress_with(
                        "finding_asset",
                        stage_progress::FINDING_ASSET_START
                            + (f64::from(candidate_index) / f64::from(total_candidates)) * asset_range,
                        u64::from(candidate_index),
                        u64::from(total_candidates),
                        format!(
                            "❌ {}/{} 失败（{}），尝试下一个...",
                            candidate_index, total_candidates, e
                        ),
                        DownloadProgressDetail {
                            step: format!("❌ {}/{} 失败", candidate_index, total_candidates),
                            step_progress: f64::from(candidate_index) / f64::from(total_candidates),
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
                u64::from(total_candidates),
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

/// 直连策略获取最新版本。
///
/// 优先走 GitHub API（单次请求即可拿到**真实**的 tag、资产名与 size，
/// 通常 < 1s），避免「猜名字 + 逐个 HEAD 探测」的长串行等待；
/// API 不可用时再回退到直连探测策略。
fn fetch_llama_latest_release_with_retry(
    _max_retries: u32,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<GitHubRelease> {
    // 策略 1：GitHub API（最快最准）
    match fetch_latest_release_via_api() {
        Ok(release) => return Ok(release),
        Err(e) => {
            tracing::warn!(
                target: "LlamaDownloader",
                error = %e,
                "GitHub API 获取失败，回退到直连探测策略"
            );
        }
    }

    // 策略 2：直连探测（API 不可用时的兜底）
    try_fetch_with_client_direct(progress_callback)
}

/// 通过 GitHub API 获取最新可用构建。
///
/// llama.cpp 的 stable release（如 v0.5.0）通常只含一个 `nightly-tag.txt`，
/// 真正的二进制在其指向的 build tag（形如 b11200）下。
///
/// 关键取舍：GitHub release API 的响应体较大（每个 release 约 67KB，
/// 因为还包含完整的 changelog markdown），而弱网下读取响应体的速度很慢
/// （实测约 10KB/s）。这里用 `per_page=5` **单次**请求取最近 5 条，
/// 一次筛选出含二进制的 build，���免二次请求。
///
/// 早期实现是 `for per_page in [1, 3]`：llama.cpp 最新一条 release 常常
/// 只含 `nightly-tag.txt` 而不含二进制，`per_page=1` 必然落空，必须再发
/// 第二次请求，版本获取耗时直接翻倍——这正是「进度条起步阶段占掉一半
/// 时间」的主要来源。
///
/// 同时刻意**不**去下载 `nightly-tag.txt`：它属于 `releases/download/` 路径，
/// 会 302 跳转到对象存储 CDN，在部分网络环境下连接会长时间挂起。
fn fetch_latest_release_via_api() -> anyhow::Result<GitHubRelease> {
    let client = Client::builder()
        // 整体超时必须覆盖「响应体读取」，弱网下 67KB 可能需要数秒
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(8))
        .user_agent("LlamaUI/0.7.0")
        .build()?;

    // 单次请求取 5 条 release，一次筛选出含二进制的版本。
    //
    // 原实现是 `for per_page in [1, 3]` 的两次串行请求：llama.cpp 的最新
    // release 常常只有 nightly-tag.txt 而不含二进制，此时 per_page=1 必然
    // 落空，必须再发第二次请求，版本获取耗时直接翻倍。
    // 改成单次 per_page=5 后，无论最新几条里哪个含二进制都能一次命中。
    let per_page = 5u32;
    let url = format!(
        "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page={}",
        per_page
    );

    // reqwest 未启用 `json` feature，这里用 serde_json 手动反序列化，
    // 避免为此新增依赖特性导致全量重编译。
    let body = match client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "LlamaDownloader",
                per_page = per_page,
                error = %e,
                "GitHub API 请求失败"
            );
            return Err(anyhow::Error::from(e));
        }
    };

    let releases: Vec<GitHubRelease> = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("解析 GitHub API 响应失败: {}", e))?;

    // 选出第一个真正带二进制的 release（跳过只含 nightly-tag.txt 的 stable tag）
    if let Some(mut rel) = releases.into_iter().find(|r| {
        r.assets
            .iter()
            .any(|a| a.name.to_lowercase().starts_with("llama-"))
    }) {
        rel.trusted_assets = true;
        tracing::info!(
            target: "LlamaDownloader",
            tag = %rel.tag_name,
            assets = rel.assets.len(),
            bytes = body.len(),
            "通过 GitHub API 获取到最新构建（资产已验证，无需 HEAD 探测）"
        );
        return Ok(rel);
    }

    Err(anyhow::anyhow!("GitHub API 未返回任何含二进制的 release"))
}

/// 获取最新版本（纯直连策略：不使用 GitHub API，避免速率限制）
/// 策略优先级：
/// 1) LLAMA_CPP_VERSION 环境变量（用户指定）
/// 2) nightly-tag.txt 直连下载（从稳定 release 获取 nightly tag）
/// 3) 硬编码已知稳定版本（回退）
fn try_fetch_with_client_direct(
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> anyhow::Result<GitHubRelease> {
    let os = current_os();
    let arch = current_arch();

    // 1) 用户指定版本（最高优先级）
    if let Ok(tag) = std::env::var("LLAMA_CPP_VERSION") {
        let tag_owned = tag.clone();
        tracing::info!(target: "LlamaDownloader", tag = %tag_owned, "使用环境变量指定的版本");
        if let Some((valid_tag, _asset_name, _)) =
            try_validate_asset_urls(&tag, &os, &arch, progress_callback)
        {
            return Ok(GitHubRelease {
                tag_name: valid_tag.clone(),
                assets: build_virtual_assets(&valid_tag, &os, &arch),
                prerelease: false,
                trusted_assets: false,
            });
        }
    }

    // 2) 直连 nightly-tag.txt 获取 nightly tag（不使用 API）
    if let Some(nightly_tag) = fetch_nightly_tag_direct() {
        tracing::warn!(
            target: "LlamaDownloader",
            nightly_tag = %nightly_tag,
            "使用 nightly-tag.txt 直连方式"
        );
        let os = current_os();
        let arch = current_arch();
        if let Some((valid_tag, _asset_name, _)) =
            try_validate_asset_urls(&nightly_tag, &os, &arch, progress_callback)
        {
            return Ok(GitHubRelease {
                tag_name: valid_tag.clone(),
                assets: build_virtual_assets(&valid_tag, &os, &arch),
                prerelease: true,
                trusted_assets: false,
            });
        }
    }

    // 3) 硬编码已知可用版本（最后兜底）
    let tag = "b11146".to_string();
    let tag_owned = tag.clone();
    tracing::warn!(target: "LlamaDownloader", tag = %tag_owned, "所有策略失败，回退到硬编码版本");
    let os = current_os();
    let arch = current_arch();
    if let Some((valid_tag, _asset_name, _)) =
        try_validate_asset_urls(&tag, &os, &arch, progress_callback)
    {
        return Ok(GitHubRelease {
            tag_name: valid_tag.clone(),
            assets: build_virtual_assets(&valid_tag, &os, &arch),
            prerelease: false,
            trusted_assets: false,
        });
    }

    Err(anyhow::anyhow!("所有策略均失败，无效的候选 URL"))
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

    // 1. 获取最新版本（直连策略，不使用 GitHub API）
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "fetching_version".to_string(),
            progress: stage_progress::FETCHING_VERSION_START,
            downloaded: 0,
            total: 0,
            message: "获取最新版本（直连策略）...".to_string(),
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
    let sha256 = compute_sha256_fast(
        &llama_server_path,
        progress_callback,
        file_size,
        cancel_token,
    )?;
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
        version: tag.clone(),
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
                        ((file_size - bytes_read) as f64 / 1_048_576.0 / speed_mbps.max(0.001))
                            as u64,
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

/// 从 GitHub release 页面直接下载 nightly-tag.txt（不使用 API）
fn fetch_nightly_tag_direct() -> Option<String> {
    // 注意：这里必须是 **llama.cpp 的 stable release tag**（vX.Y.Z），
    // 不能使用应用自身的版本号。stable release 下挂着 nightly-tag.txt，
    // 指向真正含二进制的 build tag。
    let stable_tags = [
        "v0.5.0", "v0.4.5", "v0.4.4", "v0.4.3", "v0.4.2", "v0.4.1", "v0.4.0",
    ];
    for stable_tag in &stable_tags {
        let url = format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{}/nightly-tag.txt",
            stable_tag
        );
        let mut req = shared_client().get(&url);
        req = req.timeout(std::time::Duration::from_secs(10));
        if let Some(resp) = req.send().ok() {
            if resp.status().is_success() {
                if let Ok(content) = resp.text() {
                    let tag = content.trim().to_string();
                    if !tag.is_empty() && tag.starts_with('b') {
                        tracing::info!(target: "LlamaDownloader", nightly_tag = %tag, "直接获取到 nightly tag");
                        return Some(tag);
                    }
                }
            }
        }
    }
    None
}

/// 通过 HEAD 请求验证候选 URL 是否可用
fn try_validate_asset_urls(
    tag: &str,
    os: &str,
    arch: &str,
    progress_callback: Option<&dyn Fn(DownloadProgress)>,
) -> Option<(String, String, u64)> {
    let candidates = build_official_candidate_names(tag, os, arch);
    if candidates.is_empty() {
        return None;
    }

    tracing::info!(
        target: "LlamaDownloader",
        tag = %tag,
        candidates_count = candidates.len(),
        "使用直连验证策略验证候选 URL（并行）"
    );

    let total = candidates.len();
    if let Some(cb) = progress_callback {
        cb(DownloadProgress {
            stage: "finding_asset".to_string(),
            progress: stage_progress::FINDING_ASSET_START,
            downloaded: 0,
            total: total as u64,
            message: format!("并行验证 {} 个候选安装包...", total),
            speed_mbps: 0.0,
            eta_secs: None,
            detail: Some(DownloadProgressDetail {
                step: format!("并行验证 {} 个候选", total),
                step_progress: 0.0,
                candidate_index: 0,
                candidate_count: total as u32,
                current_candidate: None,
                speed_mbps: 0.0,
                eta_secs: None,
            }),
        });
    }

    // 并行验证所有候选：一旦有任意一个可用即刻返回
    let (tx, rx) = std::sync::mpsc::channel::<(usize, String, anyhow::Result<u64>)>();
    for (i, asset_name) in candidates.iter().enumerate() {
        let asset_url = format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{}/{}",
            tag, asset_name
        );
        let tx = tx.clone();
        let name = asset_name.clone();
        std::thread::spawn(move || {
            // 短超时：HEAD 只需快速判定可用性
            let client = match Client::builder()
                .timeout(Duration::from_secs(6))
                .connect_timeout(Duration::from_secs(4))
                .user_agent("LlamaUI/0.7.0")
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send((i, name, Err(anyhow::Error::from(e))));
                    return;
                }
            };
            let res = client
                .head(&asset_url)
                .send()
                .map_err(anyhow::Error::from)
                .and_then(|resp| {
                    if !resp.status().is_success() {
                        anyhow::bail!("HTTP {} (不可用)", resp.status().as_u16());
                    }
                    let cl = resp
                        .headers()
                        .get(reqwest::header::CONTENT_LENGTH)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    Ok(cl)
                });
            let _ = tx.send((i, name, res));
        });
    }
    drop(tx);

    let mut failed = 0u32;
    while let Ok((i, asset_name, res)) = rx.recv() {
        match res {
            Ok(content_length) => {
                tracing::info!(
                    target: "LlamaDownloader",
                    tag = %tag,
                    asset_name = %asset_name,
                    content_length = content_length,
                    "✅ URL 验证通过，选中此候选"
                );
                if let Some(cb) = progress_callback {
                    cb(DownloadProgress {
                        stage: "finding_asset".to_string(),
                        progress: stage_progress::FINDING_ASSET_END,
                        downloaded: 0,
                        total: total as u64,
                        message: format!("✅ 选中：{}", asset_name),
                        speed_mbps: 0.0,
                        eta_secs: None,
                        detail: Some(DownloadProgressDetail {
                            step: format!("✅ 选中：{}", asset_name),
                            step_progress: 1.0,
                            candidate_index: (i + 1) as u32,
                            candidate_count: total as u32,
                            current_candidate: Some(asset_name.clone()),
                            speed_mbps: 0.0,
                            eta_secs: None,
                        }),
                    });
                }
                return Some((tag.to_string(), asset_name, content_length));
            }
            Err(e) => {
                failed += 1;
                tracing::debug!(
                    target: "LlamaDownloader",
                    asset_name = %asset_name,
                    error = %e,
                    "候选 URL 不可用"
                );
                if let Some(cb) = progress_callback {
                    let p = stage_progress::FINDING_ASSET_START
                        + (failed as f64 / total as f64)
                            * (stage_progress::FINDING_ASSET_END - stage_progress::FINDING_ASSET_START);
                    cb(DownloadProgress {
                        stage: "finding_asset".to_string(),
                        progress: p,
                        downloaded: 0,
                        total: total as u64,
                        message: format!("❌ {}/{} 不可用", failed, total),
                        speed_mbps: 0.0,
                        eta_secs: None,
                        detail: Some(DownloadProgressDetail {
                            step: format!("❌ {} 不可用", asset_name),
                            step_progress: failed as f64 / total as f64,
                            candidate_index: (i + 1) as u32,
                            candidate_count: total as u32,
                            current_candidate: Some(asset_name.clone()),
                            speed_mbps: 0.0,
                            eta_secs: None,
                        }),
                    });
                }
            }
        }
    }

    tracing::warn!(
        target: "LlamaDownloader",
        tag = %tag,
        failed = failed,
        "所有候选 URL 验证失败"
    );
    None
}

/// 构建"虚拟"候选资产（用于直连 URL 策略）
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

    /// 真实网络回归测试（默认忽略，用 `cargo test -- --ignored` 手动跑）。
    ///
    /// 覆盖 DEFECT：llama.cpp 的 stable release 只含 `nightly-tag.txt`，
    /// 二进制在其指向的 build tag 下；且「匹配资产」阶段必须快速返回。
    #[test]
    #[ignore = "需要访问 GitHub 网络"]
    fn test_api_release_lookup_is_fast_and_selects_real_asset() {
        let start = std::time::Instant::now();
        let release =
            fetch_latest_release_via_api().expect("应能通过 GitHub API 获取到最新构建");
        let elapsed = start.elapsed();

        assert!(!release.tag_name.is_empty(), "tag 不应为空");
        assert!(release.trusted_assets, "API 资产应标记为可信");
        assert!(!release.assets.is_empty(), "资产列表不应为空");
        assert!(
            release.assets.iter().all(|a| a.size > 0),
            "API 资产应带有真实 size"
        );

        // 该阶段曾经稳定耗时 60s+，这里锁定性能回归。
        // 注意：本机到 GitHub 的响应体带宽很低（实测约 10KB/s），
        // per_page=1 的 67KB 响应就需 ~7s，因此阈值取 30s。
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "版本获取耗时过长: {:?}",
            elapsed
        );

        // 资产匹配不应产生 HEAD 请求，直接命中
        let backend = detect_gpu_backend();
        let picked = smart_find_asset(&release, backend, None).expect("应匹配到资产");
        assert!(picked.1 > 0, "选中的资产应带真实大小");
        assert!(
            !picked.0.name.starts_with("cudart-"),
            "不应优先选择捆绑 CUDA runtime 的超大包: {}",
            picked.0.name
        );
        // 调试输出（仅用于本地验证）
        eprintln!(
            "tag={} backend={} picked={} ({:.1} MB) in {:?}",
            release.tag_name,
            backend.as_str(),
            picked.0.name,
            picked.1 as f64 / 1048576.0,
            elapsed
        );
    }

    /// 资产匹配去重：多个后端关键词命中同一资产时不应重复入列。
    #[test]
    fn test_trusted_assets_skip_head_probe() {
        let release = GitHubRelease {
            tag_name: "b1".to_string(),
            assets: vec![
                GitHubAsset {
                    name: "llama-b1-bin-win-cuda-13.4-x64.zip".to_string(),
                    browser_download_url: "https://example.com/a.zip".to_string(),
                    size: 12345,
                },
                GitHubAsset {
                    name: "cudart-llama-bin-win-cuda-13.4-x64.zip".to_string(),
                    browser_download_url: "https://example.com/b.zip".to_string(),
                    size: 99999,
                },
            ],
            prerelease: true,
            trusted_assets: true,
        };

        let picked = smart_find_asset(&release, GpuBackend::Cuda13_3, None).expect("应匹配到资产");
        // 应选普通 llama- 包（更小），而不是 cudart 打包版本
        assert_eq!(picked.0.name, "llama-b1-bin-win-cuda-13.4-x64.zip");
        assert_eq!(picked.1, 12345);
    }

    #[test]
    fn test_gpu_backend_from_str() {
        assert_eq!(GpuBackend::parse_backend("cuda"), GpuBackend::Cuda12_4);
        assert_eq!(GpuBackend::parse_backend("cuda-12.4"), GpuBackend::Cuda12_4);
        assert_eq!(GpuBackend::parse_backend("cuda-13.3"), GpuBackend::Cuda13_3);
        assert_eq!(GpuBackend::parse_backend("cuda13"), GpuBackend::Cuda13_3);
        assert_eq!(GpuBackend::parse_backend("rocm"), GpuBackend::Rocm);
        assert_eq!(GpuBackend::parse_backend("vulkan"), GpuBackend::Vulkan);
        assert_eq!(GpuBackend::parse_backend("metal"), GpuBackend::Metal);
        assert_eq!(GpuBackend::parse_backend("cpu"), GpuBackend::Cpu);
        assert_eq!(GpuBackend::parse_backend("unknown"), GpuBackend::Cpu);
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
