//! HF 模型文件下载器。
//!
//! 替换原先通过 `download_engine` / `MultiThreadDownloader` 下载 HF 模型的方式：
//! - 不再使用自研多线程分块 + 断点续传（与 HF CDN/302跳转/Token校验冲突）
//! - 改用单线程流式下载，内存友好，且更贴近浏览器行为
//! - 保留进度、取消、代理、重试等核心能力

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use reqwest::Client;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::util::proxy::read_system_proxy;
use crate::commands::hf_model_cmd::HfDownloadProgress;



/// HF 下载器。
pub struct HfDownloader {
    client: Client,
}

impl HfDownloader {
    /// 创建下载器实例。
    ///
    /// 自动注入系统代理（与现有 HF API Client 行为一致）。
    pub fn new() -> AnyResult<Self> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("LlamaUI/0.7.0");

        if let Some(proxy_url) = read_system_proxy() {
            if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
                builder = builder.proxy(proxy);
                tracing::info!(target: "HfDownloader", proxy = %proxy_url, "已注入系统代理");
            }
        }

        Ok(Self {
            client: builder.build()?,
        })
    }

    /// 执行文件下载。
    pub async fn download(
        &self,
        app: AppHandle,
        model_id: &str,
        download_id: &str,
        url: &str,
        dest_path: PathBuf,
        filename: &str,
        expected_size: u64,
    ) -> AnyResult<u64>
    {
        let mut last_downloaded: u64 = 0;
        let mut last_speed_ts = Instant::now();
        let max_retries = 3;
        let mut last_error = None;

        for attempt in 1..=max_retries {
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
    ) -> AnyResult<u64>
    {
        let start = Instant::now();
        let mut downloaded: u64 = 0;

        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(dest_path);

        let mut req = self.client.get(url);
        if expected_size > 0 && dest_path.exists() {
            let existing = std::fs::metadata(dest_path)?.len();
            if existing > 0 && existing < expected_size {
                req = req.header(reqwest::header::RANGE, format!("bytes={}-", existing));
                downloaded = existing;
                *last_downloaded = existing;
            }
        }

        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 206 {
            return Err(anyhow::anyhow!("HTTP {} 下载失败", status.as_u16()));
        }

        let content_length = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        // Emit headers stage after getting Content-Length
        let _ = app.emit(
            "hf-download-progress",
            HfDownloadProgress {
                stage: "headers".to_string(),
                progress: 0.0,
                downloaded: 0,
                total: content_length.unwrap_or(0),
                speed: None,
                eta: None,
                model_id: model_id.to_string(),
                filename: filename.to_string(),
                message: format!("获取文件信息：{}", content_length.map_or_else(|| "未知大小".to_string(), |v| format!("{} MB", v as f64 / 1024.0 / 1024.0))),
                download_id: download_id.to_string(),
            },
        );
        tracing::info!(
            target: "HfDownloader",
            url = %url,
            status = %status,
            content_length = ?content_length,
            "请求已发送，开始读取数据流"
        );

        let total = if expected_size > 0 {
            expected_size
        } else {
            resp.headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        };

        let file = if status.as_u16() == 200 && downloaded > 0 {
            downloaded = 0;
            *last_downloaded = 0;
            std::fs::File::create(dest_path)?
        } else {
            OpenOptions::new().create(true).append(true).open(dest_path)?
        };

        let mut file = file;
        let mut stream = resp.bytes_stream();

        use futures::stream::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            if downloaded % (256 * 1024) < chunk.len() as u64 || chunk.is_empty() {
                tracing::info!(
                    target: "HfDownloader",
                    downloaded = downloaded,
                    chunk = chunk.len(),
                    "收到数据块"
                );
            }

            if downloaded - *last_downloaded >= 64 * 1024 || downloaded == total {
                let progress = if total > 0 { downloaded as f64 / total as f64 } else { 0.0 };
                let now = Instant::now();
                let elapsed_secs = now.duration_since(*last_speed_ts).as_secs_f64();
                let speed = if elapsed_secs > 0.0 { ((downloaded - *last_downloaded) as f64 / elapsed_secs) as u64 } else { 0 };
                let eta = if speed > 0 && total > 0 { ((total - downloaded) / speed) as u64 } else { 0 };

                let _ = app.emit(
                    "hf-download-progress",
                    HfDownloadProgress {
                        stage: "downloading".to_string(),
                        progress,
                        downloaded,
                        total,
                        speed: Some(speed),
                        eta: Some(eta),
                        model_id: model_id.to_string(),
                        filename: filename.to_string(),
                        message: format!(
                            "{:.1} / {:.1} MB",
                            downloaded as f64 / 1_048_576.0,
                            total as f64 / 1_048_576.0
                        ),
                        download_id: download_id.to_string(),
                    },
                );

                *last_downloaded = downloaded;
                *last_speed_ts = now;
            }
        }

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

use std::io::Write;
use std::fs::OpenOptions;
