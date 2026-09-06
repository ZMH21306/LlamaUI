//! HF Model Store Commands

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};
use parking_lot::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfModelSearchResult {
    pub id: String,
    pub provider: String,
    pub model_type: String,
    pub description: Option<String>,
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    pub tags: Vec<String>,
    pub has_gguf: bool,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfModelFile {
    pub path: String,
    pub size: u64,
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfDownloadProgress {
    pub stage: String,
    pub progress: f64,
    pub downloaded: u64,
    pub total: u64,
    pub speed: Option<u64>,
    pub eta: Option<u64>,
    pub model_id: String,
    pub filename: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HfDownloadResult {
    pub path: String,
    pub file_size: u64,
    pub elapsed_ms: u64,
}

pub struct HfState {
    pub hf_token: Mutex<Option<String>>,
    pub download_dir: Mutex<PathBuf>,
}

impl HfState {
    pub fn new() -> Self {
        let default_dir = which::which("llama-server")
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("models")))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".llamaui")
                    .join("llama-cpp")
                    .join("models")
            });
        let token = std::env::var("HF_TOKEN").ok().filter(|t| !t.is_empty());
        Self {
            hf_token: Mutex::new(token),
            download_dir: Mutex::new(default_dir),
        }
    }
}

const HF_API_BASE: &str = "https://huggingface.co/api";

/// 格式化字节数为人类可读字符串。
fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

/// 同步 HTTP GET（在 spawn_blocking 中调用，避免阻塞 Tauri 事件循环）。
///
/// 返回值为 (body, http_status)。调用方应检查 status 并处理错误。
fn hf_get_sync(path: &str, token: Option<&str>) -> (String, u16) {
    let url = format!("{}{}", HF_API_BASE, path);
    let mut req = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .set("User-Agent", "LlamaUI/0.7.0")
        .set("Accept", "application/json");
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {}", t));
    }
    match req.call() {
        Ok(resp) => {
            let status = resp.status();
            match resp.into_string() {
                Ok(body) => (body, status),
                Err(e) => (format!("Read error: {}", e), 0),
            }
        }
        Err(e) => (format!("Network error: {}", e), 0),
    }
}

/// 在 spawn_blocking 中执行同步 HTTP GET，避免阻塞 Tauri 事件循环。
/// 返回 (body, http_status)；status=0 表示网络错误。
async fn hf_get(path: &str, token: Option<&str>) -> (String, u16) {
    let path_owned = path.to_string();
    let token_owned = token.map(|s| s.to_string());
    tokio::task::spawn_blocking(move || {
        hf_get_sync(&path_owned, token_owned.as_deref())
    })
    .await
    .unwrap_or_else(|_| ("Task panicked".to_string(), 0))
}

#[tauri::command]
pub async fn download_hf_model(
    app: AppHandle,
    state: State<'_, HfState>,
    model_id: String,
    filename: String,
    install_dir: Option<String>,
) -> Result<HfDownloadResult, String> {
    let dir = install_dir.map(PathBuf::from).unwrap_or_else(|| state.download_dir.lock().clone());
    fs::create_dir_all(&dir).map_err(|e| format!("Create dir failed: {}", e))?;
    let url = format!("https://huggingface.co/{}/resolve/main/{}", model_id, filename);
    let token = state.hf_token.lock().clone();
    let out_path = dir.join(&filename);
    let out_path_str = out_path.to_string_lossy().to_string();
    let start_time = std::time::Instant::now();

    let _ = app.emit("hf-download-progress", HfDownloadProgress {
        stage: "init".to_string(), progress: 0.0, downloaded: 0, total: 0,
        speed: None, eta: None,
        model_id: model_id.clone(), filename: filename.clone(),
        message: format!("Starting: {}", filename),
    });

    let url_clone = url.clone();
    let token_clone = token.clone();
    let out_clone = out_path_str.clone();
    let model_id_c = model_id.clone();
    let filename_c = filename.clone();
    let app_c = app.clone();

    let result = tokio::task::spawn_blocking(move || {
        let client = ureq::Agent::new();
        let mut req = client.get(&url_clone);
        if let Some(ref t) = token_clone {
            req = req.set("Authorization", &format!("Bearer {}", t));
        }
        let resp = req.call().map_err(|e| format!("HTTP error: {}", e))?;
        let total = resp.header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let mut reader = resp.into_reader();
        let mut file = match std::fs::File::create(&out_clone) {
            Ok(f) => f,
            Err(e) => return Err(format!("Create file failed: {}", e)),
        };
        let mut downloaded: u64 = 0;
        let mut buf = [0u8; 8192];
        let mut last_emit = std::time::Instant::now();
        let mut last_bytes = 0u64;
        let mut last_time = start_time;

        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Read error: {}", e))?;
            if n == 0 { break; }
            file.write_all(&buf[..n]).map_err(|e| format!("Write error: {}", e))?;
            downloaded += n as u64;
            let now = std::time::Instant::now();
            let elapsed_ms = now.duration_since(last_time).as_millis() as u64;
            if elapsed_ms >= 300 || downloaded == total {
                let dt = now.duration_since(last_emit).as_secs_f64();
                let speed = if dt > 0.0 { (downloaded - last_bytes) as f64 / dt } else { 0.0 };
                let remaining = if speed > 0.0 && total > downloaded {
                    (total - downloaded) as f64 / speed
                } else { 0.0 };
                let progress = if total > 0 { downloaded as f64 / total as f64 } else { 0.0 };
                let _ = app_c.emit("hf-download-progress", HfDownloadProgress {
                    stage: "downloading".to_string(),
                    progress,
                    downloaded,
                    total,
                    speed: Some(speed as u64),
                    eta: Some(remaining.ceil() as u64),
                    model_id: model_id_c.clone(),
                    filename: filename_c.clone(),
                    message: format!("{} ({}/{})", filename_c, format_size(downloaded), format_size(total)),
                });
                last_emit = now;
                last_bytes = downloaded;
                last_time = now;
            }
        }
        Ok::<(), String>(())
    }).await.map_err(|e| format!("Task panicked: {}", e))??;

    let file_size = fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    let elapsed = start_time.elapsed().as_millis() as u64;

    let _ = app.emit("hf-download-progress", HfDownloadProgress {
        stage: "complete".to_string(),
        progress: 1.0,
        downloaded: file_size,
        total: file_size,
        speed: None,
        eta: None,
        model_id: model_id.clone(),
        filename: filename.clone(),
        message: "Download complete".to_string(),
    });

    Ok(HfDownloadResult {
        path: out_path.to_string_lossy().to_string(),
        file_size,
        elapsed_ms: elapsed,
    })
}

#[tauri::command]
pub async fn search_hf_models(state: State<'_, HfState>, query: String, limit: Option<usize>) -> Result<Vec<HfModelSearchResult>, String> {
    let token = state.hf_token.lock().clone();
    let limit = limit.unwrap_or(20);
    let encoded_query = urlencoding::encode(&query);
    let url = format!("/models?search={}&limit={}&sort=downloads&direction=-1&filter=gguf&full=true", encoded_query, limit);
    let (body, status) = hf_get(&url, token.as_deref()).await;
    if status == 0 {
        return Err(format!("网络错误：无法连接到 HuggingFace API ({})。请检查网络连接或代理设置。", &body));
    }
    if status == 429 {
        return Err("请求频率超限（HuggingFace 限流）。请稍候几秒后重试，或在 Token 设置中填入 HF Token 提升配额。".to_string());
    }
    if status != 200 {
        return Err(format!("搜索失败：HTTP {} {}", status, &body));
    }
    let raw: Vec<serde_json::Value> = serde_json::from_str(&body).map_err(|e| format!("Parse failed: {}", e))?;
    if raw.is_empty() { return Ok(vec![]); }
    let mut final_results: Vec<HfModelSearchResult> = Vec::new();
    for v in raw.iter() {
        let id = match v["id"].as_str() { Some(s) => s.to_string(), None => continue };
        let siblings = v["siblings"].as_array();
        let has_gguf = siblings.map_or(false, |arr| arr.iter().any(|s| s["rfilename"].as_str().map_or(false, |f| f.ends_with(".gguf"))));
        let tags = v["tags"].as_array();
        let has_gguf_tag = tags.map_or(false, |arr| arr.iter().any(|t| t.as_str().map_or(false, |s| s.eq_ignore_ascii_case("gguf"))));
        if !has_gguf && !has_gguf_tag { continue; }
        final_results.push(HfModelSearchResult {
            id: id.clone(),
            provider: id.split('/').next().unwrap_or("unknown").to_string(),
            model_type: v["modelType"].as_str().unwrap_or("model").to_string(),
            description: v["description"].as_str().map(|s| s.to_string()),
            downloads: v["downloads"].as_u64(), likes: v["likes"].as_u64(),
            tags: tags.map(|arr| arr.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect()).unwrap_or_default(),
            has_gguf: true, last_modified: v["lastModified"].as_str().map(|s| s.to_string()),
        });
    }
    Ok(final_results)
}

#[tauri::command]
pub async fn get_hf_model_files(state: State<'_, HfState>, model_id: String) -> Result<Vec<HfModelFile>, String> {
    let token = state.hf_token.lock().clone();
    let encoded_id = model_id.split('/').map(|s| urlencoding::encode(s)).collect::<Vec<_>>().join("/");
    let (body, status) = hf_get(&format!("/models/{}", &encoded_id), token.as_deref()).await;
    if status == 0 {
        return Err(format!("网络错误：无法连接到 HuggingFace API ({})。请检查网络连接或代理设置。", &body));
    }
    if status == 429 {
        return Err("请求频率超限（HuggingFace 限流）。请稍候几秒后重试，或在 Token 设置中填入 HF Token 提升配额。".to_string());
    }
    if status != 200 {
        return Err(format!("获取文件失败：HTTP {} {}", status, &body));
    }
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("Parse failed: {}", e))?;
    let siblings = v["siblings"].as_array().ok_or_else(|| format!("No siblings for {}", model_id))?;
    let files: Vec<HfModelFile> = siblings.iter().filter_map(|s| {
        let rfilename = s["rfilename"].as_str()?;
        if !rfilename.ends_with(".gguf") { return None; }
        Some(HfModelFile { path: rfilename.to_string(), size: s["size"].as_u64().unwrap_or(0), r#type: s["type"].as_str().unwrap_or("blob").to_string() })
    }).collect();
    Ok(files)
}

#[tauri::command]
pub fn set_hf_token(state: State<'_, HfState>, token: Option<String>) { *state.hf_token.lock() = token; }

#[tauri::command]
pub fn get_hf_token(state: State<'_, HfState>) -> Option<String> { state.hf_token.lock().clone() }

#[tauri::command]
pub fn set_hf_download_dir(state: State<'_, HfState>, dir: String) { *state.download_dir.lock() = PathBuf::from(dir); }

#[tauri::command]
pub fn get_hf_download_dir(state: State<'_, HfState>) -> String { state.download_dir.lock().to_string_lossy().to_string() }

#[tauri::command]
pub async fn open_hf_store_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("hf-store") {
        window.show().map_err(|e| format!("Show window failed: {}", e))?;
        window.set_focus().map_err(|e| format!("Focus window failed: {}", e))?;
        return Ok(());
    }
    let main_window = app.get_webview_window("main").ok_or_else(|| "Main window not found")?;
    use tauri::WebviewWindowBuilder;
    let url = tauri::WebviewUrl::App("hf-store.html".into());
    let builder = WebviewWindowBuilder::new(&app, "hf-store", url)
        .title("HuggingFace Model Store")
        .inner_size(920.0, 720.0)
        .min_inner_size(760.0, 560.0)
        .resizable(true)
        .center();
    let builder = builder.parent(&main_window).map_err(|e| format!("Set parent failed: {}", e))?;
    let window = builder.build().map_err(|e| format!("Build window failed: {}", e))?;
    window.show().map_err(|e| format!("Show window failed: {}", e))?;
    Ok(())
}

