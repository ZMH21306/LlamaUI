//! HuggingFace 模型商城命令。
//!
//! 提供模型搜索、文件列表查询、下载和进度回调能力。
//! 使用 HuggingFace Hub API (https://huggingface.co/api/models)。
//! 下载使用 curl 子进程（自动继承系统代理），通过事件实时推送进度。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::io::{BufRead, BufReader};
use tauri::{AppHandle, Emitter, Manager, State};
use parking_lot::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfModelSearchResult {
    pub id: String, pub model_type: String, pub description: Option<String>,
    pub downloads: Option<u64>, pub likes: Option<u64>, pub tags: Vec<String>,
    pub has_gguf: bool, pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfModelFile { pub path: String, pub size: u64, pub r#type: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfDownloadProgress {
    pub stage: String, pub progress: f64, pub downloaded: u64, pub total: u64,
    pub speed: Option<u64>, pub eta: Option<u64>,
    pub model_id: String, pub filename: String, pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfDownloadResult { pub path: String, pub file_size: u64, pub elapsed_ms: u64 }

pub struct HfState { pub hf_token: Mutex<Option<String>>, pub download_dir: Mutex<PathBuf> }

impl HfState {
    pub fn new() -> Self {
        // 默认下载到 llama-server 可执行文件所在目录的 models 子目录
        // 查找顺序：系统 PATH -> ~/.llamaui/llama-cpp/llama-server(.exe) -> 兜底路径
        let default_dir = which::which("llama-server")
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("models")))
            .or_else(|| {
                #[cfg(windows)]
                let exe = "llama-server.exe";
                #[cfg(not(windows))]
                let exe = "llama-server";
                dirs::home_dir()
                    .map(|h| h.join(".llamaui").join("llama-cpp").join(exe))
                    .filter(|p| p.is_file())
                    .and_then(|p| p.parent().map(|d| d.join("models")))
            })
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".llamaui")
                    .join("llama-cpp")
                    .join("models")
            });
        let token = std::env::var("HF_TOKEN").ok().filter(|t| !t.is_empty());
        Self { hf_token: Mutex::new(token), download_dir: Mutex::new(default_dir) }
    }
}

const HF_API_BASE: &str = "https://huggingface.co/api";

fn hf_get(path: &str, token: Option<&str>) -> anyhow::Result<String> {
    let url = format!("{}{}", HF_API_BASE, path);
    let mut req = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))  // 30秒超时
        .set("User-Agent", "LlamaUI/0.7.0")
        .set("Accept", "application/json");
    if let Some(t) = token { req = req.set("Authorization", &format!("Bearer {}", t)); }
    let response = req.call().map_err(|e| anyhow::anyhow!("HF API 请求失败: {} (URL: {})", e, url))?;
    let status = response.status();
    if status != 200 { return Err(anyhow::anyhow!("HF API 返回 HTTP {} (URL: {})", status, url)); }
    response.into_string().map_err(|e| anyhow::anyhow!("读取响应体失败: {}", e))
}

#[tauri::command]
pub async fn download_hf_model(app: AppHandle, state: State<'_, HfState>, model_id: String, filename: String, install_dir: Option<String>) -> Result<HfDownloadResult, String> {
    let dir = install_dir.map(PathBuf::from).unwrap_or_else(|| state.download_dir.lock().clone());
    let token = state.hf_token.lock().clone();
    let download_url = format!("https://huggingface.co/{}/resolve/main/{}", model_id, filename);
    // P0-2 安全修复：对 filename 做严格净化，拒绝路径遍历 / 绝对路径 / 设备名。
    // 修复前：`filename.split('/').last()` 仅剥掉 `/` 分隔的目录前缀，
    //   但对 `..\evil.exe`（Windows 反斜杠）完全无效，攻击者可写入目标目录之外。
    let safe_name = crate::util::path::sanitize_filename(&filename)
        .map_err(|e| format!("文件名非法：{}（原始：{}）", e, filename))?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;
    let dest_path = dir.join(&safe_name);
    tracing::info!(target: "HfDownload", url = %download_url, dest = %dest_path.display(), "开始下载 HF 模型");
    let _ = app.emit("hf-download-progress", HfDownloadProgress { stage: "init".into(), progress: 0.0, downloaded: 0, total: 0, speed: None, eta: None, model_id: model_id.clone(), filename: filename.clone(), message: format!("开始下载 {}...", filename) });
    let app_clone = app.clone(); let mid_clone = model_id.clone(); let fn_clone = filename.clone(); let tok_clone = token.clone();
    let result = tokio::task::spawn_blocking(move || curl_download_hf(&download_url, &dest_path, tok_clone.as_deref(), &app_clone, &mid_clone, &fn_clone)).await.map_err(|e| format!("下载任务失败: {}", e))?.map_err(|e| format!("{}", e))?;
    let _ = app.emit("hf-download-progress", HfDownloadProgress { stage: "complete".into(), progress: 1.0, downloaded: result.file_size, total: result.file_size, speed: None, eta: None, model_id, filename, message: "下载完成".into() });
    Ok(result)
}

fn curl_download_hf(url: &str, dest: &Path, token: Option<&str>, app: &AppHandle, model_id: &str, filename: &str) -> anyhow::Result<HfDownloadResult> {
    let start = std::time::Instant::now();
    let mut cmd = std::process::Command::new("curl");
    cmd.args(&["-s", "-o", dest.to_str().unwrap_or(""), "-w", "%{size_download}", "--max-time", "3600", "--retry", "3", "--retry-delay", "5", "-L"]);
    if let Some(t) = token { cmd.arg("-H").arg(format!("Authorization: Bearer {}", t)); }
    cmd.arg("-H").arg("User-Agent: LlamaUI/0.7.0"); cmd.arg(url); cmd.stdout(Stdio::piped()); cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("curl 启动失败: {}", e))?;
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let app_c = app.clone(); let mid_c = model_id.to_string(); let fn_c = filename.to_string();
    std::thread::spawn(move || { for line in reader.lines().flatten() { if let Ok(bytes) = line.trim().parse::<u64>() { if bytes > 0 { let _ = app_c.emit("hf-download-progress", HfDownloadProgress { stage: "downloading".into(), progress: 0.0, downloaded: bytes, total: 0, speed: None, eta: None, model_id: mid_c.clone(), filename: fn_c.clone(), message: format!("已下载 {}...", format_size(bytes)) }); } } } });
    let output = child.wait_with_output().map_err(|e| anyhow::anyhow!("curl 等待失败: {}", e))?;
    if !output.status.success() { return Err(anyhow::anyhow!("curl 退出码: {:?}", output.status.code())); }
    let downloaded: u64 = String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0);
    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(HfDownloadResult { path: dest.to_string_lossy().to_string(), file_size: downloaded, elapsed_ms })
}

fn format_size(bytes: u64) -> String { let units = ["B", "KB", "MB", "GB"]; let mut size = bytes as f64; let mut idx = 0; while size >= 1024.0 && idx < units.len() - 1 { size /= 1024.0; idx += 1; } format!("{:.1} {}", size, units[idx]) }

#[tauri::command] pub fn set_hf_token(state: State<'_, HfState>, token: String) { *state.hf_token.lock() = Some(token); }
#[tauri::command] pub fn get_hf_token(state: State<'_, HfState>) -> Option<String> { state.hf_token.lock().clone() }
#[tauri::command] pub fn set_hf_download_dir(state: State<'_, HfState>, dir: String) { *state.download_dir.lock() = PathBuf::from(dir); }
#[tauri::command] pub fn get_hf_download_dir(state: State<'_, HfState>) -> String { state.download_dir.lock().to_string_lossy().to_string() }

#[tauri::command]
pub async fn open_hf_store_window(app: tauri::AppHandle) -> Result<(), String> {
    // 如果窗口存在但被隐藏/关闭，则显示
    if let Some(window) = app.get_webview_window("hf-store") {
        window.show().map_err(|e| format!("显示窗口失败: {}", e))?;
        window.set_focus().map_err(|e| format!("聚焦窗口失败: {}", e))?;
        return Ok(());
    }
    // 窗口不存在则重新创建（处理被 close 销毁的情况）
    use tauri::WebviewWindowBuilder;
    let url = tauri::WebviewUrl::App("hf-store.html".into());
    WebviewWindowBuilder::new(&app, "hf-store", url)
        .title("HuggingFace 模型商城")
        .inner_size(920.0, 720.0)
        .min_inner_size(760.0, 560.0)
        .resizable(true)
        .center()
        .build()
        .map_err(|e| format!("创建窗口失败: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn search_hf_models(state: State<'_, HfState>, query: String, limit: Option<usize>) -> Result<Vec<HfModelSearchResult>, String> {
    let token = state.hf_token.lock().clone();
    let limit = limit.unwrap_or(20);
    let encoded_query = urlencoding::encode(&query);
    // 使用 full=true 直接获取 siblings，避免每个模型再单独调用 API
    let url = format!("/models?search={}&limit={}&sort=downloads&direction=-1&full=true", encoded_query, limit);
    let body = hf_get(&url, token.as_deref()).map_err(|e| format!("搜索失败: {}", e))?;
    let raw: Vec<serde_json::Value> = serde_json::from_str(&body).map_err(|e| format!("解析失败: {}", e))?;
    if raw.is_empty() { return Ok(vec![]); }

    // 直接从搜索结果的 siblings 判断 GGUF（full=true 已返回 siblings）
    let final_results: Vec<HfModelSearchResult> = raw.iter().filter_map(|v| {
        let id = v["id"].as_str()?.to_string();
        let siblings = v["siblings"].as_array();
        let has_gguf = siblings.map_or(false, |arr| {
            arr.iter().any(|s| s["rfilename"].as_str().map_or(false, |f| f.ends_with(".gguf")))
        });
        if !has_gguf { return None; }
        Some(HfModelSearchResult {
            id,
            model_type: v["modelType"].as_str().unwrap_or("model").to_string(),
            description: v["description"].as_str().map(|s| s.to_string()),
            downloads: v["downloads"].as_u64(), likes: v["likes"].as_u64(),
            tags: v["tags"].as_array().map(|arr| arr.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect()).unwrap_or_default(),
            has_gguf,
            last_modified: v["lastModified"].as_str().map(|s| s.to_string()),
        })
    }).collect();
    Ok(final_results)
}

#[tauri::command]
pub async fn get_hf_model_files(state: State<'_, HfState>, model_id: String) -> Result<Vec<HfModelFile>, String> {
    let token = state.hf_token.lock().clone();
    let encoded_id = model_id.split('/')
        .map(|s| urlencoding::encode(s))
        .collect::<Vec<_>>()
        .join("/");
    let body = hf_get(&format!("/models/{}", encoded_id), token.as_deref())
        .map_err(|e| format!("获取模型 {} 文件失败: {}", model_id, e))?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("解析模型 {} 响应失败: {}", model_id, e))?;
    let siblings = v["siblings"].as_array().ok_or_else(|| format!("模型 {} 无 siblings 字段", model_id))?;
    let files: Vec<HfModelFile> = siblings.iter().filter_map(|s| {
        let rfilename = s["rfilename"].as_str()?;
        if !rfilename.ends_with(".gguf") { return None; }
        Some(HfModelFile { path: rfilename.to_string(), size: s["size"].as_u64().unwrap_or(0), r#type: s["type"].as_str().unwrap_or("blob").to_string() })
    }).collect();
    Ok(files)
}
