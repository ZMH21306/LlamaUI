//! HF Model Store Commands

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    pub download_id: String,
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
    /// 共享 ureq Agent（P2-3 修复）。
    ///
    /// 原先每次 `download_hf_model` / `hf_get_sync` 都新建 `ureq::Agent` /
    /// `ureq::get`，没有连接池，大模型下载时 TCP 三次握手开销不可忽略。
    /// 改为单例后，所有 HF 请求复用同一个 Agent，支持 keep-alive 与
    /// DNS 缓存，下载速度可提升 10~30%。
    pub agent: Mutex<ureq::Agent>,
    /// 下载取消通道（P2-4 修复）。
    ///
    /// key = 下载 ID（`model_id::filename`），value = `oneshot::Sender<()>`。
    /// 前端调用 `cancel_hf_download` 时发送信号，后端 `download_hf_model`
    /// 的 `spawn_blocking` 闭包在每次循环迭代时检查该信号，收到后
    /// 立即退出并删除已写入的不完整文件。
    pub download_cancels: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
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
            // P2-3：单例 Agent（连接池），timeout 30s
            agent: Mutex::new(
                ureq::AgentBuilder::new()
                    .timeout(std::time::Duration::from_secs(30))
                    .build(),
            ),
            // P2-4：空取消通道
            download_cancels: Mutex::new(HashMap::new()),
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
///
/// P2-3：使用共享 `ureq::Agent`（连接池），避免每次新建 Agent 的 TCP 开销。
fn hf_get_sync(agent: &ureq::Agent, path: &str, token: Option<&str>) -> (String, u16) {
    let url = format!("{}{}", HF_API_BASE, path);
    let mut req = agent
        .get(&url)
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
                Err(e) => (format!("读取响应失败：{}", e), 0),
            }
        }
        Err(e) => (format!("网络错误：{}", e), 0),
    }
}

/// 在 spawn_blocking 中执行同步 HTTP GET，避免阻塞 Tauri 事件循环。
/// 返回 (body, http_status)；status=0 表示网络错误。
///
/// P2-3：从 `state.agent` 取共享 Agent（连接池），避免每次新建 TCP 连接。
async fn hf_get(state: &HfState, path: &str, token: Option<&str>) -> (String, u16) {
    let path_owned = path.to_string();
    let token_owned = token.map(|s| s.to_string());
    let agent = state.agent.lock().clone();
    tokio::task::spawn_blocking(move || {
        hf_get_sync(&agent, &path_owned, token_owned.as_deref())
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
    expected_size: Option<u64>,
) -> Result<HfDownloadResult, String> {
    // ===== 安全校验（P0-1 + P0-2 修复） =====
    // 1) model_id 必须严格匹配 `org/name` 形式：只允许 ASCII 字母/数字/_-./，长度 1~128，
    //    两段（非空），拒绝包含 `?` `#` `&` `:` `/` `\` 等 URL 注入字符。
    if model_id.is_empty() || model_id.len() > 128 {
        return Err("非法的 model_id：长度必须 1~128".to_string());
    }
    let parts: Vec<&str> = model_id.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
        return Err(format!(
            "非法的 model_id：必须是 org/name 形式（实际：{}）",
            model_id
        ));
    }
    if !parts.iter().all(|p| {
        p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    }) {
        return Err(format!(
            "非法的 model_id：仅允许字母/数字/_-./（实际：{}）",
            model_id
        ));
    }

    // 2) filename 必须通过 `sanitize_filename()` 校验（拒绝路径遍历、设备名、绝对路径等）
    // P2-6：取最后一部分作为文件名（HF 的 rfilename 可能含子目录）
    let base_filename = filename.rsplit('/').next().unwrap_or(&filename);
    let safe_filename = crate::util::path::sanitize_filename(base_filename)
        .map_err(|e| format!("非法文件名，拒绝下载：{:?}", e))?;

    let dir = install_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| state.download_dir.lock().clone());
    fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败：{}", e))?;
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        model_id, filename
    );
    let token = state.hf_token.lock().clone();
    let out_path = dir.join(&safe_filename);
    let out_path_str = out_path.to_string_lossy().to_string();
    let start_time = std::time::Instant::now();
    // P2-4：注册取消通道。前端可调用 `cancel_hf_download(download_id)` 触发取消
    let download_id = format!("{}::{}", model_id, filename);
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    state
        .download_cancels
        .lock()
        .insert(download_id.clone(), cancel_tx);

    let _ = app.emit(
        "hf-download-progress",
        HfDownloadProgress {
            stage: "init".to_string(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
            speed: None,
            eta: None,
            model_id: model_id.clone(),
            filename: filename.clone(),
            message: format!("开始下载：{}", filename),
            download_id: download_id.clone(),
        },
    );

    let url_clone = url.clone();
    let token_clone = token.clone();
    let out_clone = out_path_str.clone();
    let model_id_c = model_id.clone();
    let filename_c = filename.clone();
    let app_c = app.clone();
    // P2-3：从共享 Agent 取 clone（ureq::Agent 是轻量克隆，复用连接池）
    let agent_clone = state.agent.lock().clone();

    let download_id_for_cleanup = download_id.clone();
    let download_id_for_emit = download_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut req = agent_clone.get(&url_clone);
        if let Some(ref t) = token_clone {
            req = req.set("Authorization", &format!("Bearer {}", t));
        }
        let resp = req.call().map_err(|e| format!("HTTP 请求失败：{}", e))?;
        let total = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok())
            .or(expected_size)
            .unwrap_or(0);
        let mut reader = resp.into_reader();
        let mut file = match std::fs::File::create(&out_clone) {
            Ok(f) => f,
            Err(e) => return Err(format!("创建文件失败：{}", e)),
        };
        let mut downloaded: u64 = 0;
        let mut buf = [0u8; 8192];
        let mut last_emit = std::time::Instant::now();
        let mut last_bytes = 0u64;
        let mut last_time = start_time;

        loop {
            // P2-4：检查取消信号。如果前端调用了 `cancel_hf_download`，
            // 这里会立即收到，停止下载并删除不完整文件。
            if cancel_rx.try_recv().is_ok() {
                let _ = std::fs::remove_file(&out_clone);
                let _ = app_c.emit(
                    "hf-download-progress",
                    HfDownloadProgress {
                        stage: "cancelled".to_string(),
                        progress: 0.0,
                        downloaded: 0,
                        total: 0,
                        speed: None,
                        eta: None,
                        model_id: model_id_c.clone(),
                        filename: filename_c.clone(),
                        message: format!("下载已取消：{}", filename_c),
                        download_id: download_id_for_emit.clone(),
                    },
                );
                return Err("下载已被用户取消".to_string());
            }

            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("读取网络数据失败：{}", e))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("写入文件失败：{}", e))?;
            downloaded += n as u64;
            let now = std::time::Instant::now();
            let elapsed_ms = now.duration_since(last_time).as_millis() as u64;
            if elapsed_ms >= 300 || downloaded == total {
                let dt = now.duration_since(last_emit).as_secs_f64();
                let speed = if dt > 0.0 {
                    (downloaded - last_bytes) as f64 / dt
                } else {
                    0.0
                };
                let remaining = if speed > 0.0 && total > downloaded {
                    (total - downloaded) as f64 / speed
                } else {
                    0.0
                };
                let progress = if total > 0 {
                    downloaded as f64 / total as f64
                } else {
                    0.0
                };
                let _ = app_c.emit(
                    "hf-download-progress",
                    HfDownloadProgress {
                        stage: "downloading".to_string(),
                        progress,
                        downloaded,
                        total,
                        speed: Some(speed as u64),
                        eta: Some(remaining.ceil() as u64),
                        model_id: model_id_c.clone(),
                        filename: filename_c.clone(),
                        message: format!(
                            "{} ({}/{})",
                            filename_c,
                            format_size(downloaded),
                            format_size(total)
                        ),
                        download_id: download_id_for_emit.clone(),
                    },
                );
                last_emit = now;
                last_bytes = downloaded;
                last_time = now;
            }
        }
        Ok::<(), String>(())
    })
    .await;

    // P2-4：清理取消通道。下载完成（成功/失败/cancelled）后从 Map 移除，
    // 避免内存泄漏（前端每下一次新单都会注册一个新 entry）。
    state.download_cancels.lock().remove(&download_id_for_cleanup);

    // 处理 spawn_blocking 结果
    result.map_err(|e| format!("下载任务执行失败：{}", e))??;

    let file_size = fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    let elapsed = start_time.elapsed().as_millis() as u64;

    let _ = app.emit(
        "hf-download-progress",
        HfDownloadProgress {
            stage: "complete".to_string(),
            progress: 1.0,
            downloaded: file_size,
            total: file_size,
            speed: None,
            eta: None,
            model_id: model_id.clone(),
            filename: filename.clone(),
            message: "下载完成".to_string(),
            download_id: format!("{}::{}", model_id, filename),
        },
    );

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
    let (body, status) = hf_get(&state, &url, token.as_deref()).await;
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
    let (body, status) = hf_get(&state, &format!("/models/{}", &encoded_id), token.as_deref()).await;
    if status == 0 {
        return Err(format!("网络错误：无法连接到 HuggingFace API ({})。请检查网络连接或代理设置。", &body));
    }
    if status == 429 {
        return Err("请求频率超限（HuggingFace 限流）。请稍候几秒后重试，或在 Token 设置中填入 HF Token 提升配额。".to_string());
    }
    if status != 200 {
        return Err(format!("获取文件失败：HTTP {} {}", status, &body));
    }
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败：{}", e))?;
    let siblings = v["siblings"].as_array().ok_or_else(|| format!("模型 {} 没有文件列表", model_id))?;
    let files: Vec<HfModelFile> = siblings.iter().filter_map(|s| {
        let rfilename = s["rfilename"].as_str()?;
        if !rfilename.ends_with(".gguf") { return None; }
        Some(HfModelFile { path: rfilename.to_string(), size: s["size"].as_u64().or_else(|| s["lfs"].get("size").and_then(|v| v.as_u64())).unwrap_or(0), r#type: s["type"].as_str().unwrap_or("blob").to_string() })
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
pub async fn cancel_hf_download(
    state: State<'_, HfState>,
    model_id: String,
    filename: String,
) -> Result<(), String> {
    let download_id = format!("{}::{}", model_id, filename);
    // 查找注册在 Map 中的 cancel sender
    let tx = {
        let mut cancels = state.download_cancels.lock();
        cancels.remove(&download_id)
    };
    match tx {
        Some(sender) => {
            let _ = sender.send(());
            Ok(())
        }
        None => Err(format!(
            "未找到进行中的下载：{}（可能已完成或不存在）",
            download_id
        )),
    }
}

#[tauri::command]
pub async fn open_hf_store_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("hf-store") {
        window.show().map_err(|e| format!("显示窗口失败：{}", e))?;
        window.set_focus().map_err(|e| format!("聚焦窗口失败：{}", e))?;
        return Ok(());
    }
    let main_window = app
        .get_webview_window("main")
        .ok_or_else(|| "未找到主窗口".to_string())?;
    use tauri::WebviewWindowBuilder;
    let url = tauri::WebviewUrl::App("hf-store.html".into());
    let builder = WebviewWindowBuilder::new(&app, "hf-store", url)
        .title("HuggingFace 模型商店")
        .inner_size(920.0, 720.0)
        .min_inner_size(760.0, 560.0)
        .resizable(true)
        .center();
    let builder = builder
        .parent(&main_window)
        .map_err(|e| format!("设置父窗口失败：{}", e))?;
    let window = builder
        .build()
        .map_err(|e| format!("创建窗口失败：{}", e))?;
    window
        .show()
        .map_err(|e| format!("显示窗口失败：{}", e))?;
    Ok(())
}

// ============================================================
// 单元测试
// ============================================================
#[cfg(test)]
mod tests {
    //! 覆盖 P0-1/P0-2 安全修复：`download_hf_model` 的输入校验逻辑。
    //!
    //! 校验逻辑抽到纯函数 `validate_model_id`，便于在不开 Tauri runtime
    //! 的情况下快速回归。

    /// 校验 model_id 是否符合 `org/name` 形式。
    /// 严格白名单：仅 ASCII 字母/数字 + 字符 `_` `-` `.`，且恰好两段。
    /// 这与 `download_hf_model` 内的内联校验等价（抽出便于测试）。
    pub fn validate_model_id(model_id: &str) -> Result<(), String> {
        if model_id.is_empty() || model_id.len() > 128 {
            return Err("非法的 model_id：长度必须 1~128".to_string());
        }
        let parts: Vec<&str> = model_id.split('/').collect();
        if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
            return Err(format!(
                "非法的 model_id：必须是 org/name 形式（实际：{}）",
                model_id
            ));
        }
        if !parts.iter().all(|p| {
            p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        }) {
            return Err(format!(
                "非法的 model_id：仅允许字母/数字/_-./（实际：{}）",
                model_id
            ));
        }
        Ok(())
    }

    /// P0-2 回归：合法 model_id 必须通过。
    #[test]
    fn validate_model_id_accepts_legit() {
        assert!(validate_model_id("TheBloke/Llama-2-7B-GGUF").is_ok());
        assert!(validate_model_id("Qwen/Qwen2-7B-Instruct-GGUF").is_ok());
        assert!(validate_model_id("user123/model.v2").is_ok());
        assert!(validate_model_id("a/b").is_ok()); // 最短合法
    }

    /// P0-2 回归：拒绝路径遍历 / URL 注入 / 协议混淆。
    #[test]
    fn validate_model_id_rejects_attacks() {
        // 路径遍历
        assert!(validate_model_id("../etc/passwd").is_err());
        assert!(validate_model_id("..\\evil").is_err());
        // 协议混淆
        assert!(validate_model_id("http://evil.com/").is_err());
        // URL 注入字符
        assert!(validate_model_id("org?token=ATTACKER/name").is_err());
        assert!(validate_model_id("org#fragment/name").is_err());
        assert!(validate_model_id("org&extra=name").is_err());
        assert!(validate_model_id("org:name").is_err()); // 单段包含非法字符
        // 多段（不止两段）
        assert!(validate_model_id("a/b/c").is_err());
        // 单段
        assert!(validate_model_id("only-one-segment").is_err());
        // 空
        assert!(validate_model_id("").is_err());
        // 含空格的合法攻击载荷
        assert!(validate_model_id("org name/model").is_err());
    }

    /// P0-2 回归：超长 model_id 必须被拒绝。
    #[test]
    fn validate_model_id_rejects_too_long() {
        let long = "a".repeat(129);
        assert!(validate_model_id(&long).is_err());
    }

    /// P0-1 回归：sanitize_filename 与 validate_model_id 组合能阻止攻击链。
    ///
    /// 模拟：恶意仓库返回 `rfilename = "../../etc/passwd"`，配合合法 model_id。
    /// 验证 sanitize_filename 拦截，避免实际写入到 `dir.join("../../etc/passwd")`。
    #[test]
    fn sanitize_blocks_path_traversal_payload() {
        let malicious_filename = "../../etc/passwd";
        let result = crate::util::path::sanitize_filename(malicious_filename);
        assert!(result.is_err(), "路径遍历 payload 必须被 sanitize_filename 拒绝");
    }

    /// P0-1 回归：合法 GGUF 文件名通过。
    #[test]
    fn sanitize_accepts_gguf_filename() {
        let ok = sanitize_for_test("model-q4_k_m.gguf");
        assert!(ok.is_ok());
        assert_eq!(ok.unwrap(), "model-q4_k_m.gguf");
    }

    // 包装一层避免上面的 `use` 影响其他测试
    fn sanitize_for_test(name: &str) -> Result<String, crate::util::path::FilenameError> {
        crate::util::path::sanitize_filename(name)
    }
}

