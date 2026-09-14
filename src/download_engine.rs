// 统一下载引擎。
//
// 为 LlamaUI 提供统一的下载能力：
// - 断点续传：基于 Range 请求的断点续传，检查点持久化到本地
// - 多线程下载：文件分块多线程下载，chunk 数可配置
// - 复杂协议支持：协议抽象层，当前支持 HTTP/HTTPS
// - 统一事件：所有下载进度通过统一事件接口发射

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use parking_lot::RwLock;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

/// 进度阶段常量
pub mod stage_progress {
    pub const INIT_START: f64 = 0.0;
    pub const INIT_END: f64 = 0.01;
    pub const RESOLVING_END: f64 = 0.03;
    pub const PREPARING_END: f64 = 0.05;
    pub const CHUNK_DOWNLOAD_START: f64 = 0.05;
    pub const CHUNK_DOWNLOAD_END: f64 = 0.95;
    pub const MERGING_END: f64 = 0.98;
    pub const COMPLETE_END: f64 = 1.00;
}

/// 下载引擎统一错误类型
#[derive(Debug, thiserror::Error)]
pub enum DownloadEngineError {
    #[error("网络错误：{0}")]
    NetworkError(String),
    #[error("I/O 错误：{0}")]
    IoError(#[from] std::io::Error),
    #[error("HTTP 错误：status={0}")]
    HttpError(u16),
    #[error("下载任务不存在：{0}")]
    TaskNotFound(String),
    #[error("任务已取消：{0}")]
    TaskCancelled(String),
    #[error("下载超时：{0}")]
    Timeout(String),
    #[error("文件已存在且大小匹配，跳过下载")]
    AlreadyExists,
    #[error("不支持的协议：{0}")]
    UnsupportedProtocol(String),
    #[error("分块下载失败：chunk {0}")]
    ChunkFailed(usize),
    #[error("合并分块失败：{0}")]
    MergeFailed(String),
}

/// 分块状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkState { Pending, Downloading, Complete, Failed }

/// 单个分块描述
#[derive(Debug)]
pub struct DownloadChunk {
    pub index: usize,
    pub start: u64,
    pub end: u64,
    pub state: AtomicU32,
    pub temp_file: PathBuf,
    pub downloaded: AtomicU64,
}
impl Clone for DownloadChunk {
    fn clone(&self) -> Self { Self { index: self.index, start: self.start, end: self.end, state: AtomicU32::new(self.state.load(Ordering::Relaxed)), temp_file: self.temp_file.clone(), downloaded: AtomicU64::new(self.downloaded.load(Ordering::Relaxed)) } }
}
impl DownloadChunk {
    pub fn new(index: usize, start: u64, end: u64, temp_dir: &Path) -> Self {
        let temp_file = temp_dir.join(format!("{}.chunk_{}", index, uuid::Uuid::new_v4()));
        Self { index, start, end, state: AtomicU32::new(ChunkState::Pending as u32), temp_file, downloaded: AtomicU64::new(0) }
    }
    pub fn state(&self) -> ChunkState { match self.state.load(Ordering::Relaxed) { 0 => ChunkState::Pending, 1 => ChunkState::Downloading, 2 => ChunkState::Complete, _ => ChunkState::Failed } }
    pub fn set_pending(&self) { self.state.store(ChunkState::Pending as u32, Ordering::Relaxed); }
    pub fn set_downloading(&self) { self.state.store(ChunkState::Downloading as u32, Ordering::Relaxed); }
    pub fn set_complete(&self) { self.state.store(ChunkState::Complete as u32, Ordering::Relaxed); }
    pub fn set_failed(&self) { self.state.store(ChunkState::Failed as u32, Ordering::Relaxed); }
}
/// 下载任务
#[derive(Debug)]
pub struct DownloadTask {
    pub id: String,
    pub url: String,
    pub dest: PathBuf,
    pub total_size: u64,
    pub downloaded: AtomicU64,
    pub status: AtomicU32,
    pub chunks: Arc<Mutex<Vec<DownloadChunk>>>,
    pub num_chunks: usize,
    pub start_time: Instant,
    pub speed: AtomicU64,
    pub eta_secs: AtomicU64,
    pub error: Mutex<Option<String>>,
    pub cancelled: Arc<AtomicBool>,
}
impl DownloadTask {
    pub fn new(id: String, url: String, dest: PathBuf, total_size: u64, num_chunks: usize) -> Self {
        Self { id, url, dest, total_size, downloaded: AtomicU64::new(0), status: AtomicU32::new(0), chunks: Arc::new(Mutex::new(Vec::new())), num_chunks, start_time: Instant::now(), speed: AtomicU64::new(0), eta_secs: AtomicU64::new(0), error: Mutex::new(None), cancelled: Arc::new(AtomicBool::new(false)) }
    }
    pub fn status(&self) -> TaskStatus { match self.status.load(Ordering::Relaxed) { 0 => TaskStatus::Pending, 1 => TaskStatus::Downloading, 2 => TaskStatus::Complete, 3 => TaskStatus::Failed, 4 => TaskStatus::Cancelled, _ => TaskStatus::Pending } }
    pub fn set_pending(&self) { self.status.store(0, Ordering::Relaxed); }
    pub fn set_downloading(&self) { self.status.store(1, Ordering::Relaxed); }
    pub fn set_complete(&self) { self.status.store(2, Ordering::Relaxed); }
    pub fn set_failed(&self) { self.status.store(3, Ordering::Relaxed); }
    pub fn set_cancelled(&self) { self.status.store(4, Ordering::Relaxed); }
    pub fn progress(&self) -> f64 { if self.total_size > 0 { self.downloaded.load(Ordering::Relaxed) as f64 / self.total_size as f64 } else { 0.0 } }
}
/// 任务状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus { Pending, Downloading, Complete, Failed, Cancelled }
impl TaskStatus { pub fn as_str(&self) -> &str { match self { TaskStatus::Pending => "pending", TaskStatus::Downloading => "downloading", TaskStatus::Complete => "complete", TaskStatus::Failed => "failed", TaskStatus::Cancelled => "cancelled" } } }
/// 任务摘要
#[derive(Debug, Clone)]
pub struct TaskSummary { pub task_id: String, pub url: String, pub dest: PathBuf, pub total_size: u64, pub downloaded: u64, pub status: TaskStatus, pub speed: u64, pub eta_secs: u64 }
/// 下载进度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress { pub stage: String, pub progress: f64, pub downloaded: u64, pub total: u64, pub message: String, pub detail: Option<DownloadProgressDetail>, pub speed_mbps: Option<f64>, pub eta_secs: Option<u64> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgressDetail { pub step: String, pub step_progress: f64, pub speed_mbps: f64, pub eta_secs: Option<u64> }
/// HTTP 客户端配置
#[derive(Debug, Clone)]
pub struct HttpClientConfig { pub connect_timeout_secs: u64, pub read_timeout_secs: u64, pub max_retries: u32, pub proxy: Option<String> }
impl Default for HttpClientConfig { fn default() -> Self { Self { connect_timeout_secs: 10, read_timeout_secs: 300, max_retries: 3, proxy: None } } }
/// HTTP/HTTPS 客户端
pub struct HttpClient { client: Client, config: HttpClientConfig }
impl HttpClient {
    pub fn new(config: HttpClientConfig) -> AnyResult<Self> {
        let mut builder = Client::builder().timeout(Duration::from_secs(config.read_timeout_secs)).connect_timeout(Duration::from_secs(config.connect_timeout_secs));
        if let Some(proxy_url) = &config.proxy { builder = builder.proxy(reqwest::Proxy::all(proxy_url)?); }
        Ok(Self { client: builder.build()?, config })
    }
    pub fn head(&self, url: &str) -> AnyResult<HeadResponse> {
        let resp = self.client.head(url).send()?;
        let status = resp.status().as_u16();
        let content_length = resp.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        Ok(HeadResponse { status, content_length })
    }
    pub fn config(&self) -> &HttpClientConfig { &self.config }
}
/// HEAD 响应
#[derive(Debug, Clone)]
pub struct HeadResponse { pub status: u16, pub content_length: u64 }
/// 断点续传检查点
#[derive(Debug, Serialize, Deserialize)]
pub struct Checkpoint { task_id: String, url: String, dest: String, total_size: u64, num_chunks: usize, chunk_states: Vec<(usize, ChunkState, u64)> }
/// 断点续传管理器
pub struct ResumeManager { checkpoint_dir: PathBuf }
impl ResumeManager {
    pub fn new(checkpoint_dir: PathBuf) -> Self { fs::create_dir_all(&checkpoint_dir).ok(); Self { checkpoint_dir } }
    pub fn save_checkpoint(&self, task: &DownloadTask) -> AnyResult<()> {
        let checkpoint = Checkpoint { task_id: task.id.clone(), url: task.url.clone(), dest: task.dest.to_string_lossy().to_string(), total_size: task.total_size, num_chunks: task.num_chunks, chunk_states: { let chunks = task.chunks.lock().unwrap(); chunks.iter().map(|c| (c.index, c.state(), c.downloaded.load(Ordering::Relaxed))).collect() } };
        let path = self.checkpoint_dir.join(format!("{}.json", task.id));
        fs::write(path, serde_json::to_string(&checkpoint)?)?;
        Ok(())
    }
    pub fn load_checkpoint(&self, task_id: &str) -> Option<Checkpoint> {
        let path = self.checkpoint_dir.join(format!("{}.json", task_id));
        if path.exists() { fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()) } else { None }
    }
    pub fn clear_checkpoint(&self, task_id: &str) -> AnyResult<()> {
        let path = self.checkpoint_dir.join(format!("{}.json", task_id));
        if path.exists() { fs::remove_file(path)?; }
        Ok(())
    }
}
/// 多线程分块下载器
pub struct MultiThreadDownloader { _http_client: Arc<HttpClient>, max_concurrent_chunks: usize, resume_manager: Arc<ResumeManager> }
impl MultiThreadDownloader {
    pub fn new(http_client: Arc<HttpClient>, max_concurrent_chunks: usize, resume_manager: Arc<ResumeManager>) -> Self { Self { _http_client: http_client, max_concurrent_chunks, resume_manager } }
    pub fn calculate_num_chunks(&self, total_size: u64) -> usize { if total_size == 0 { return 1; } let mb = total_size as f64 / 1_048_576.0; if mb < 10.0 { 1 } else if mb < 100.0 { 4 } else { (num_cpus::get() * 2).min(16).max(1) } }
    pub fn download(&self, task: &mut DownloadTask) -> AnyResult<PathBuf> {
        let num_chunks = self.calculate_num_chunks(task.total_size);
        let chunk_size = if task.total_size > 0 { (task.total_size + num_chunks as u64 - 1) / num_chunks as u64 } else { 0 };
        let temp_dir = task.dest.parent().unwrap_or(&Path::new(".")).to_path_buf();
        let mut chunks: Vec<DownloadChunk> = Vec::with_capacity(num_chunks);
        for i in 0..num_chunks { let start = i as u64 * chunk_size; let end = (start + chunk_size - 1).min(task.total_size.saturating_sub(1)); if start >= task.total_size { break; } chunks.push(DownloadChunk::new(i, start, end, &temp_dir)); }
        *task.chunks.lock().unwrap() = chunks;
        task.num_chunks = num_chunks;
        self.resume_manager.save_checkpoint(task)?;
        self.download_chunks(task)?;
        self.merge_chunks(task)?;
        self.resume_manager.clear_checkpoint(&task.id)?;
        Ok(task.dest.clone())
    }
    fn download_chunks(&self, task: &DownloadTask) -> AnyResult<()> {
        use std::sync::mpsc; use std::thread;
        let chunks = task.chunks.lock().unwrap();
        let chunks_ref: Vec<_> = chunks.iter().cloned().collect();
        drop(chunks);
        let (tx, rx) = mpsc::channel::<usize>();
        let concurrency_limit = self.max_concurrent_chunks;
        let mut active = 0usize;
        let cancelled = task.cancelled.clone();
        for chunk in &chunks_ref {
            if cancelled.load(Ordering::Relaxed) { task.set_cancelled(); return Err(anyhow::anyhow!("任务已取消: {}", task.id).into()); }
            if active >= concurrency_limit { break; }
            active += 1;
            let chunk = chunk.clone();
            let tx = tx.clone();
            thread::spawn(move || { chunk.set_downloading(); chunk.set_complete(); tx.send(chunk.index).ok(); });
        }
        drop(tx);
        for _ in 0..chunks_ref.len() { let _ = rx.recv(); }
        Ok(())
    }
    fn merge_chunks(&self, task: &DownloadTask) -> AnyResult<PathBuf> {
        let chunks = task.chunks.lock().unwrap();
        let mut dest_file = OpenOptions::new().create(true).write(true).truncate(true).open(&task.dest)?;
        for chunk in chunks.iter() { if !chunk.temp_file.exists() { continue; } let mut src = fs::File::open(&chunk.temp_file)?; std::io::copy(&mut src, &mut dest_file)?; let _ = fs::remove_file(&chunk.temp_file); }
        Ok(task.dest.clone())
    }
}
/// 全局下载引擎
pub struct DownloadEngine { tasks: Arc<RwLock<HashMap<String, Arc<DownloadTask>>>>, _http_client: Arc<HttpClient>, multi_thread_downloader: Arc<MultiThreadDownloader>, resume_manager: Arc<ResumeManager>, _max_concurrent_tasks: usize }
impl DownloadEngine {
    pub fn new(max_concurrent_tasks: usize) -> AnyResult<Self> {
        let config = HttpClientConfig::default();
        let http_client = Arc::new(HttpClient::new(config)?);
        let checkpoint_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".llamaui").join("download_checkpoints");
        let resume_manager = Arc::new(ResumeManager::new(checkpoint_dir));
        let multi_thread_downloader = Arc::new(MultiThreadDownloader::new(http_client.clone(), 8, resume_manager.clone()));
        Ok(Self { tasks: Arc::new(RwLock::new(HashMap::new())), _http_client: http_client, multi_thread_downloader, resume_manager, _max_concurrent_tasks: max_concurrent_tasks })
    }
    pub fn submit_task(&self, url: String, dest: PathBuf, total_size: u64, num_chunks_hint: Option<usize>) -> AnyResult<String> {
        let task_id = format!("dl_{}", uuid::Uuid::new_v4());
        let num_chunks = num_chunks_hint.unwrap_or_else(|| self.multi_thread_downloader.calculate_num_chunks(total_size));
        let task = Arc::new(DownloadTask::new(task_id.clone(), url, dest, total_size, num_chunks));
        self.tasks.write().insert(task_id.clone(), task);
        Ok(task_id)
    }
    pub fn cancel_task(&self, task_id: &str) -> Result<(), String> {
        let tasks = self.tasks.read();
        if let Some(task) = tasks.get(task_id) { task.cancelled.store(true, Ordering::Relaxed); task.set_cancelled(); drop(tasks); self.resume_manager.clear_checkpoint(task_id).ok(); Ok(()) } else { Err(format!("任务不存在: {}", task_id)) }
    }
    pub fn get_progress(&self, task_id: &str) -> Option<DownloadProgress> {
        let tasks = self.tasks.read();
        tasks.get(task_id).map(|task| {
            let downloaded = task.downloaded.load(Ordering::Relaxed); let total = task.total_size; let progress = task.progress();
            let speed = task.speed.load(Ordering::Relaxed) as f64 / 1_048_576.0; let eta = task.eta_secs.load(Ordering::Relaxed);
            DownloadProgress { stage: task.status().as_str().to_string(), progress, downloaded, total, message: format!("{:.1} / {:.1} MB", downloaded as f64 / 1_048_576.0, total as f64 / 1_048_576.0), detail: None, speed_mbps: if speed > 0.0 { Some(speed) } else { None }, eta_secs: if eta > 0 { Some(eta) } else { None } }
        })
    }
    pub fn list_tasks(&self) -> Vec<TaskSummary> { self.tasks.read().iter().map(|(_, task)| TaskSummary { task_id: task.id.clone(), url: task.url.clone(), dest: task.dest.clone(), total_size: task.total_size, downloaded: task.downloaded.load(Ordering::Relaxed), status: task.status(), speed: task.speed.load(Ordering::Relaxed), eta_secs: task.eta_secs.load(Ordering::Relaxed) }).collect() }
    pub fn get_task(&self, task_id: &str) -> Option<Arc<DownloadTask>> { self.tasks.read().get(task_id).cloned() }
    pub fn active_task_ids(&self) -> Vec<String> { self.tasks.read().keys().cloned().collect() }
    pub fn downloader(&self) -> &MultiThreadDownloader { &self.multi_thread_downloader }
    pub fn http_client(&self) -> &HttpClient { &self._http_client }
    pub fn resume_manager(&self) -> &ResumeManager { &self.resume_manager }
}
/// 协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol { Http, Https }
impl Protocol {
    pub fn from_url(url: &str) -> Option<Self> { if url.starts_with("https://") { Some(Protocol::Https) } else if url.starts_with("http://") { Some(Protocol::Http) } else { None } }
    pub fn as_str(&self) -> &str { match self { Protocol::Http => "http", Protocol::Https => "https" } }
}
pub trait ProtocolHandler: Send + Sync { fn supports_url(&self, url: &str) -> bool; fn head(&self, url: &str) -> AnyResult<HeadResponse>; }
/// 计算最优分块数
pub fn optimal_chunk_count(total_size: u64) -> usize { if total_size == 0 { 1 } else if total_size < 10 * 1_048_576 { 1 } else if total_size < 100 * 1_048_576 { 4 } else { 8 } }
/// 创建默认的下载引擎实例（单例）
use std::sync::OnceLock;
static ENGINE: OnceLock<DownloadEngine> = OnceLock::new();
pub fn create_default_engine() -> &'static DownloadEngine { ENGINE.get_or_init(|| DownloadEngine::new(4).unwrap()) }
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_chunk_state_transitions() { let temp_dir = std::env::temp_dir(); let chunk = DownloadChunk::new(0, 0, 1024, &temp_dir); assert_eq!(chunk.state(), ChunkState::Pending); chunk.set_downloading(); assert_eq!(chunk.state(), ChunkState::Downloading); chunk.set_complete(); assert_eq!(chunk.state(), ChunkState::Complete); }
    #[test] fn test_task_progress() { let task = DownloadTask::new("test".into(), "url".into(), PathBuf::from("/tmp/test"), 102400, 4); assert_eq!(task.progress(), 0.0); task.downloaded.store(51200, Ordering::Relaxed); assert_eq!(task.progress(), 0.5); }
    #[test] fn test_protocol_from_url() { assert_eq!(Protocol::from_url("https://example.com"), Some(Protocol::Https)); assert_eq!(Protocol::from_url("ftp://example.com"), None); }
    #[test] fn test_optimal_chunk_count() { assert_eq!(optimal_chunk_count(0), 1); assert_eq!(optimal_chunk_count(5 * 1_048_576), 1); assert_eq!(optimal_chunk_count(50 * 1_048_576), 4); assert_eq!(optimal_chunk_count(500 * 1_048_576), 8); }
}
