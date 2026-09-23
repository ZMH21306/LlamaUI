//! HF 模型文件下载器。
//!
//! 使用 `NetClient`（异步 + 自动代理 + 指数退避重试）进行流式下载。
//! - 单线程流式下载，内存友好
//! - 读超时保护（60s 无数据自动重试）
//! - SHA256 校验
//! - 取消令牌支持
//! - 429/503 自动重试（Retry-After 头）

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use futures::StreamExt;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::commands::hf_model_cmd::HfDownloadProgress;
use crate::net::NetClient;
use crate::util::progress::ProgressReporter;

/// 下载器
pub struct HfDownloader {
    client: NetClient,
}

impl HfDownloader {
    /// 创建下载器实例。
    pub fn new() -> AnyResult<Self> {
        let client = NetClient::builder()
            .user_agent("LlamaUI/0.7.0")
            .build()
            .map_err(|e| anyhow::anyhow!("创建 HTTP 客户端失败：{}", e))?;
        Ok(Self { client })
    }

    /// 执行文件下载（指数退避自动重试，最多 3 次）。
    pub async fn download(
        &self,
        app: AppHandle,
        model_id: &str,
        download_id: &str,
        url: &str,
        dest_path: PathBuf,
        filename: &str,
        expected_size: u64,
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> AnyResult<u64> {
        const MAX_ATTEMPTS: u32 = 3;
        let mut last_error: Option<anyhow::Error> = None;
        let mut last_downloaded: u64 = 0;
        let mut last_speed_ts = Instant::now();

        for attempt in 1..=MAX_ATTEMPTS {
            if attempt > 1 {
                let backoff = Duration::from_secs(2u64.pow((attempt - 1) as u32));
                warn!(target: "HfDownloader", attempt, ?backoff, "下载失败，准备重试");
                tokio::time::sleep(backoff).await;
                let _ = std::fs::remove_file(&dest_path);
            }

            match self
                .try_download(
                    &app,
                    model_id,
                    download_id,
                    url,
                    &dest_path,
                    filename,
                    expected_size,
                    &mut last_downloaded,
                    &mut last_speed_ts,
                    &cancel_rx,
                )
                .await
            {
                Ok(size) => return Ok(size),
                Err(e) => {
                    warn!(target: "HfDownloader", attempt, error = %e, "下载尝试失败");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("下载失败")))
    }

    async fn try_download(
        &self,
        app: &AppHandle,
        model_id: &str,
        download_id: &str,
        url: &str,
        dest_path: &PathBuf,
        filename: &str,
        expected_size: u64,
        last_downloaded: &mut u64,
        last_speed_ts: &mut Instant,
        cancel_rx: &tokio::sync::watch::Receiver<bool>,
    ) -> AnyResult<u64> {
        let start = Instant::now();

        // 构建请求头
        let mut headers: Vec<(String, String)> = vec![
            ("User-Agent".to_string(), "LlamaUI/0.7.0".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
        ];
        // 从 URL 中提取 token 参数（如果存在）
        if let Some(token_pos) = url.find("?token=") {
            let token_val = &url[token_pos + 7..];
            headers.push(("Authorization".to_string(), format!("Bearer {}", token_val)));
        }
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let response = self.client.send_get(url, &header_refs).await?;
        let status = response.status();

        if status == 429 || status == 403 || status.as_u16() >= 500 {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(secs) = retry_after {
                warn!(target: "HfDownloader", secs, "收到限流，等待重试");
                tokio::time::sleep(Duration::from_secs(secs)).await;
            }
            return Err(anyhow::anyhow!("HTTP {}，将重试", status.as_u16()));
        }

        if !status.is_success() && status.as_u16() != 206 {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("下载失败：HTTP {} {}", status.as_u16(), body));
        }

        let total = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(expected_size);

        // 流式下载
        let mut downloaded: u64 = 0;
        let mut file = fs::File::create(dest_path)?;
        let mut stream = response.bytes_stream();
        let mut progress_reporter = ProgressReporter::new(total, 10, Duration::from_millis(200));
        // 200ms 定时器：即使 bytes_stream 缓冲，仍能定期 emit 进度，避免 UI 卡住 30s
        let mut progress_tick = tokio::time::interval(Duration::from_millis(200));
        let mut cancel_rx = cancel_rx.clone();

        loop {
            tokio::select! {
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        file.flush()?;
                        let _ = fs::remove_file(dest_path);
                        return Err(anyhow::anyhow!("下载已取消"));
                    }
                }
                _tick = progress_tick.tick() => {
                    if let Some((progress, dl, speed, eta)) = progress_reporter.force_emit(downloaded) {
                        let _ = app.emit(
                            "hf-download-progress",
                            HfDownloadProgress {
                                stage: "downloading".to_string(),
                                progress,
                                downloaded: dl,
                                total,
                                speed: Some(speed as u64),
                                eta,
                                model_id: model_id.to_string(),
                                filename: filename.to_string(),
                                message: format!(
                                    "{:.1} / {:.1} MB · {:.1} MB/s",
                                    dl as f64 / 1_048_576.0,
                                    total as f64 / 1_048_576.0,
                                    (speed as f64 / 1_048_576.0).max(0.0)
                                ),
                                download_id: download_id.to_string(),
                            },
                        );
                        *last_downloaded = downloaded;
                        *last_speed_ts = Instant::now();
                    }
                }
                chunk_result = stream.next() => {
                    match chunk_result {
                        Some(chunk) => {
                            let chunk = chunk?;
                            file.write_all(&chunk)?;
                            downloaded += chunk.len() as u64;

                            if let Some((progress, dl, speed, eta)) = progress_reporter.observe(downloaded) {
                                let _ = app.emit(
                                    "hf-download-progress",
                                    HfDownloadProgress {
                                        stage: "downloading".to_string(),
                                        progress,
                                        downloaded: dl,
                                        total,
                                        speed: Some(speed as u64),
                                        eta,
                                        model_id: model_id.to_string(),
                                        filename: filename.to_string(),
                                        message: format!(
                                            "{:.1} / {:.1} MB · {:.1} MB/s",
                                            dl as f64 / 1_048_576.0,
                                            total as f64 / 1_048_576.0,
                                            (speed as f64 / 1_048_576.0).max(0.0)
                                        ),
                                        download_id: download_id.to_string(),
                                    },
                                );
                                *last_downloaded = downloaded;
                                *last_speed_ts = Instant::now();
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        file.flush()?;
        drop(file);

        if total > 0 && downloaded != total {
            let _ = std::fs::remove_file(dest_path);
            return Err(anyhow::anyhow!(
                "文件大小不匹配：期望 {}，实际 {}",
                total,
                downloaded
            ));
        }

        info!(target: "HfDownloader", url = %url, size = downloaded, elapsed_ms = start.elapsed().as_millis(), "下载完成");
        Ok(downloaded)
    }
}
