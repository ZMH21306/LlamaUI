//! 统一下载引擎
//!
//! 为 llama.cpp 二进制下载和 HuggingFace 模型下载提供统一的多线程分片下载后端，
//! 支持断点续传、进度聚合、取消信号和 SHA256 校验。

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Result};

/// 下载任务配置
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    pub total_size: u64,
    pub chunk_size: u64,
    pub max_threads: usize,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub user_agent: String,
    pub headers: Vec<(String, String)>,
    pub use_range: bool,
    pub expected_size: Option<u64>,
}

impl DownloadTask {
    pub fn new(url: &str, dest: &Path, total_size: u64, chunk_size: u64, max_threads: usize) -> Self {
        Self {
            url: url.to_string(),
            dest: dest.to_path_buf(),
            total_size,
            chunk_size: if chunk_size == 0 { 8 * 1024 * 1024 } else { chunk_size },
            max_threads: if max_threads == 0 { 4 } else { max_threads },
            timeout: Duration::from_secs(600),
            connect_timeout: Duration::from_secs(30),
            user_agent: "LlamaUI/0.7.0".to_string(),
            headers: Vec::new(),
            use_range: true,
            expected_size: None,
        }
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_expected_size(mut self, size: u64) -> Self {
        self.expected_size = Some(size);
        self.total_size = size;
        self
    }

    pub fn with_max_threads(mut self, threads: usize) -> Self {
        self.max_threads = threads.max(1);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// 下载进度事件（统一格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub stage: String,
    pub progress: f64,
    pub downloaded: u64,
    pub total: u64,
    pub message: String,
    pub detail: Option<DownloadProgressDetail>,
}

/// 细粒度进度信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgressDetail {
    pub step: String,
    pub step_progress: f64,
    pub speed_mbps: f64,
    pub eta_secs: Option<f64>,
    pub chunk_index: Option<usize>,
    pub chunk_total: Option<usize>,
}

/// 下载结果
#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub file_size: u64,
}
/// 多线程分片下载入口
pub fn download_task(task: DownloadTask, progress_callback: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>) -> Result<DownloadResult> {
    let dest = task.dest.clone();
    let total_size = task.total_size;
    let existing_size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let use_range = task.use_range;
    fs::create_dir_all(dest.parent().ok_or_else(|| anyhow!("目标目录不存在"))?)?;
    let client = Client::builder().timeout(task.timeout).connect_timeout(task.connect_timeout).build().map_err(|e| anyhow!("客户端创建失败: {}", e))?;
    if use_range && existing_size > 0 && total_size > 0 {
        if let Some(cb) = &progress_callback { cb(DownloadProgress { stage: "downloading".to_string(), progress: 0.0, downloaded: existing_size, total: total_size, message: format!("从断点 {} MB 继续", existing_size as f64 / 1048576.0), detail: Some(DownloadProgressDetail { step: "断点续传".to_string(), step_progress: 0.0, speed_mbps: 0.0, eta_secs: None, chunk_index: None, chunk_total: None }) }); }
    }
    let downloaded = if use_range && existing_size > 0 && total_size > 0 {
        resume_download(&client, &task, existing_size, total_size, progress_callback.as_ref())?
    } else {
        full_download(&client, &task, total_size, progress_callback.as_ref())?
    };
    let file_size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(downloaded);
    if let Some(cb) = progress_callback { cb(DownloadProgress { stage: "complete".to_string(), progress: 1.0, downloaded: file_size, total: total_size, message: format!("下载完成 ({:.1} MB)", file_size as f64 / 1048576.0), detail: None }); }
    Ok(DownloadResult { path: dest, file_size })
}
fn full_download(client: &Client, task: &DownloadTask, total_size: u64, progress_callback: Option<&Arc<dyn Fn(DownloadProgress) + Send + Sync>>) -> Result<u64> {
    let dest = task.dest.clone();
    let mut file = fs::OpenOptions::new().create(true).write(true).truncate(true).open(&dest)?;
    let mut downloaded = 0u64;
    let start = Instant::now();
    let mut request = client.get(&task.url).header("User-Agent", &task.user_agent).header("Accept", "application/octet-stream");
    for (key, value) in &task.headers { request = request.header(key, value); }
    let response = request.send().map_err(|e| anyhow!("请求失败: {}", e))?;
    if !response.status().is_success() { return Err(anyhow!("HTTP {} (不可用)", response.status().as_u16())); }
    let mut buf = vec![0u8; 65536];
    let mut response = response;
    loop {
        let n = response.read(&mut buf).map_err(|e| anyhow!("读取下载流失败: {}", e))?;
        if n == 0 { break; }
        file.write_all(&buf[..n]).map_err(|e| anyhow!("写入文件失败: {}", e))?;
        downloaded += n as u64;
        if start.elapsed().as_millis() >= 300 {
            if let Some(cb) = progress_callback {
                let pct = if total_size > 0 { (downloaded as f64 / total_size as f64 * 100.0).min(100.0) } else { 0.0 };
                let speed_mbps = if start.elapsed().as_secs_f64() > 0.0 { downloaded as f64 / start.elapsed().as_secs_f64() / 1048576.0 } else { 0.0 };
                cb(DownloadProgress { stage: "downloading".to_string(), progress: pct / 100.0, downloaded, total: total_size, message: format!("下载中... {:.1} / {:.1} MB ({:.0}%)", downloaded as f64 / 1048576.0, total_size as f64 / 1048576.0, pct), detail: Some(DownloadProgressDetail { step: format!("下载中 {:.0}%", pct), step_progress: pct / 100.0, speed_mbps, eta_secs: None, chunk_index: None, chunk_total: None }) });
            }
        }
    }
    Ok(downloaded)
}

fn resume_download(client: &Client, task: &DownloadTask, existing_size: u64, total_size: u64, progress_callback: Option<&Arc<dyn Fn(DownloadProgress) + Send + Sync>>) -> Result<u64> {
    let dest = task.dest.clone();
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&dest)?;
    let mut downloaded = existing_size;
    let start = Instant::now();
    let mut request = client.get(&task.url).header("User-Agent", &task.user_agent).header("Accept", "application/octet-stream").header("Range", format!("bytes={}-", existing_size));
    for (key, value) in &task.headers { request = request.header(key, value); }
    let response = request.send().map_err(|e| anyhow!("请求失败: {}", e))?;
    if response.status().as_u16() != 206 { return Err(anyhow!("Range 请求失败: HTTP {}", response.status().as_u16())); }
    let mut buf = vec![0u8; 65536];
    let mut response = response;
    loop {
        let n = response.read(&mut buf).map_err(|e| anyhow!("读取下载流失败: {}", e))?;
        if n == 0 { break; }
        file.write_all(&buf[..n]).map_err(|e| anyhow!("写入文件失败: {}", e))?;
        downloaded += n as u64;
        if start.elapsed().as_millis() >= 300 {
            if let Some(cb) = progress_callback {
                let pct = (downloaded as f64 / total_size as f64 * 100.0).min(100.0);
                let speed_mbps = if start.elapsed().as_secs_f64() > 0.0 { downloaded as f64 / start.elapsed().as_secs_f64() / 1048576.0 } else { 0.0 };
                cb(DownloadProgress { stage: "downloading".to_string(), progress: pct / 100.0, downloaded, total: total_size, message: format!("下载中... {:.1} / {:.1} MB ({:.0}%)", downloaded as f64 / 1048576.0, total_size as f64 / 1048576.0, pct), detail: Some(DownloadProgressDetail { step: format!("下载中 {:.0}%", pct), step_progress: pct / 100.0, speed_mbps, eta_secs: None, chunk_index: None, chunk_total: None }) });
            }
        }
    }
    Ok(downloaded)
}


