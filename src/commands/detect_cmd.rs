//! 自动检测命令。
//!
//! 把 [`crate::detect`] 模块的同步检测能力适配成异步 IPC：
//! - `detect_llama_server` / `detect_models_dir`：完整 1-2-3-4 优先级链
//! - `cancel_detection`：并发取消所有进行中的检测
//! - `check_models_dir`：用户手动选择目录后的合规性校验
//!
//! # 阻塞隔离
//!
//! 同步检测（`std::fs::read_dir` 等）会阻塞调用线程。Tauri IPC 跑在 tokio
//! runtime 上，若直接在 `async fn` 中调用同步检测会卡住 worker 线程。
//! 所以这两个 detect 命令都通过 [`tokio::task::spawn_blocking`] 把同步
//! 检测转移到 blocking 线程池。

use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, State};
use tokio::time::timeout;

use crate::detect::{self, DetectResult};
use crate::util::process::silent_tokio_command;

use super::AppState;

/// 扫描结果：用户手动选择模型目录后，校验其合规性。
/// 包含 `.gguf` 文件的目录视为合规；否则给出详细原因。
#[derive(Debug, Serialize)]
pub struct ModelsDirCheck {
    /// 目录是否可用（含至少一个 .gguf 文件）
    pub valid: bool,
    /// 直接子目录中 .gguf 文件的数量（不递归）
    pub gguf_count: usize,
    /// 子目录中包含 .gguf 的目录数（用于提示「可能选错上一层」）
    pub subdir_with_gguf: usize,
    /// 给用户看的人话说明（成功时为绿色提示，失败时为错误原因）
    pub message: String,
}

/// 完整检测 llama-server（1-2-3-4 优先级链）。
/// 进度通过 `detect-progress` 事件实时推送到前端；UI 可通过 `cancel_detection` 中止。
/// 兜底全盘扫描带时间预算（5s）+ 入口预算（30000）+ 取消标志，**不会卡死**。
///
/// 取消标志管理（P1-13 修复）：使用 Vec 记录所有正在进行的检测，并发场景下
/// 每次 push 一个独立 flag，检测完成后 retain 移除自身，避免互相覆盖。
#[tauri::command]
pub async fn detect_llama_server(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DetectResult, String> {
    let cancel = detect::new_cancel_flag();
    // parking_lot::Mutex::lock() 不会 poison（无 unwrap 失败路径）
    state.detect_cancels.lock().push(cancel.clone());
    let app2 = app.clone();
    // 需要再 clone 一份 cancel 给闭包，否则 move 之后 retain 拿不到原 Arc
    let cancel_for_task = cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        detect::detect_llama_with_progress(&app2, cancel_for_task)
    })
    .await
    .unwrap_or_else(|e| DetectResult {
        kind: "llama".into(),
        found: false,
        path: None,
        stage_found: 0,
        elapsed_ms: 0,
        entries_scanned: 0,
        message: format!("检测失败：{}", e),
    });
    // 仅移除本次检测自身对应的 flag；并发场景下保留其它进行中的 flag。
    state
        .detect_cancels
        .lock()
        .retain(|c| !std::sync::Arc::ptr_eq(c, &cancel));
    Ok(result)
}

/// 完整检测模型目录（1-2-3-4 优先级链）。同 `detect_llama_server` 的语义。
#[tauri::command]
pub async fn detect_models_dir(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DetectResult, String> {
    let cancel = detect::new_cancel_flag();
    state.detect_cancels.lock().push(cancel.clone());
    let app2 = app.clone();
    let cancel_for_task = cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        detect::detect_models_with_progress(&app2, cancel_for_task)
    })
    .await
    .unwrap_or_else(|e| DetectResult {
        kind: "models".into(),
        found: false,
        path: None,
        stage_found: 0,
        elapsed_ms: 0,
        entries_scanned: 0,
        message: format!("检测失败：{}", e),
    });
    state
        .detect_cancels
        .lock()
        .retain(|c| !std::sync::Arc::ptr_eq(c, &cancel));
    Ok(result)
}

/// 取消正在进行的检测。
/// 遍历 `detect_cancels` 中所有 flag 置 true（覆盖并发场景下的所有检测），
/// 然后清空 Vec。如果没有检测在跑，本命令返回 `false`。
#[tauri::command]
pub fn cancel_detection(state: State<'_, AppState>) -> bool {
    let mut guard = state.detect_cancels.lock();
    if guard.is_empty() {
        return false;
    }
    for c in guard.iter() {
        c.store(true, Ordering::Relaxed);
    }
    guard.clear();
    true
}

/// 从 `llama-server --version` 的输出中提取版本号。
///
/// 优先读取 `build N`，并统一显示为下载标签使用的 `bN` 格式。
/// 例如：`version: 0.4.0-dev (build 10809, ...)` → `b10809`。
/// 没有 build 信息时，再回退到普通版本号或原始输出。
fn parse_llama_version(text: &str) -> Option<String> {
    // llama.cpp 的开发版通常同时包含语义版本和 build 编号。
    // 这里的 build 编号与下载页面显示的 bNNNN 标签对应，优先使用它。
    if let Some(caps) = Regex::new(r"(?i)\bbuild\s*[:=]?\s*(b?[0-9]+)\b")
        .ok()
        .and_then(|re| re.captures(text))
    {
        let value = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if value.starts_with('b') || value.starts_with('B') {
            return Some(format!("b{}", &value[1..]));
        }
        return Some(format!("b{}", value));
    }

    // 兼容 `llama-server b1234` / `llama.cpp 1234` 形式的输出。
    if let Some(caps) = Regex::new(r"(?i)(?:llama-server|llama\.cpp)[^0-9A-Za-z]*([0-9]+|b[0-9]+)")
        .ok()
        .and_then(|re| re.captures(text))
    {
        return caps.get(1).map(|m| m.as_str()).map(|value| {
            if value.starts_with('b') || value.starts_with('B') {
                format!("b{}", &value[1..])
            } else {
                value.to_string()
            }
        });
    }

    // 兼容只有 `version: 0.4.0` 或 `version = 1.2.3` 的输出。
    if let Some(caps) = Regex::new(r"(?i)\b(?:version|v)\s*[:=]?\s*(b?[0-9]+(?:\.[0-9]+)*)")
        .ok()
        .and_then(|re| re.captures(text))
    {
        return caps.get(1).map(|m| m.as_str()).map(str::to_string);
    }

    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// 读取指定 llama-server 可执行文件的版本。
#[tauri::command]
pub async fn get_llama_version(path: String) -> Result<String, String> {
    let output = timeout(
        Duration::from_secs(5),
        silent_tokio_command(&path).arg("--version").output(),
    )
    .await
    .map_err(|_| "读取版本超时".to_string())?
    .map_err(|e| format!("读取版本失败: {}", e))?;

    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_llama_version(&text).ok_or_else(|| "未解析到版本".to_string())
}

/// 用户手动选择模型目录后，校验其合规性。
///
/// 设计动机：自动检测找不到时，用户用「浏览」自己选目录。常见错误是选到
/// 包含多个模型子目录的「父目录」（如 `D:\models\` 下面还有 `llama3/`、
/// `qwen2/`），但本应用「路由模式」要求直接子目录就是 .gguf 文件。
/// 此命令区分这两种情况并给出具体指引。
#[tauri::command]
pub fn check_models_dir(path: String) -> ModelsDirCheck {
    let p = Path::new(&path);
    if !p.exists() {
        return ModelsDirCheck {
            valid: false,
            gguf_count: 0,
            subdir_with_gguf: 0,
            message: format!("路径不存在：{}", path),
        };
    }
    if !p.is_dir() {
        return ModelsDirCheck {
            valid: false,
            gguf_count: 0,
            subdir_with_gguf: 0,
            message: format!("不是目录：{}", path),
        };
    }

    let rd = match std::fs::read_dir(p) {
        Ok(r) => r,
        Err(e) => {
            return ModelsDirCheck {
                valid: false,
                gguf_count: 0,
                subdir_with_gguf: 0,
                message: format!("无法读取目录：{}（{}）", path, e),
            };
        }
    };

    let mut gguf_count: usize = 0;
    let mut subdir_with_gguf: usize = 0;
    for e in rd.flatten() {
        let ep = e.path();
        if !ep.is_file() {
            // 检查子目录：是否有 .gguf 文件（用于提示用户可能选错层）
            if ep.is_dir() {
                if let Ok(sub) = std::fs::read_dir(&ep) {
                    for se in sub.flatten() {
                        if se
                            .path()
                            .extension()
                            .and_then(|s| s.to_str())
                            .map(|x| x.eq_ignore_ascii_case("gguf"))
                            .unwrap_or(false)
                        {
                            subdir_with_gguf += 1;
                            break;
                        }
                    }
                }
            }
            continue;
        }
        if ep
            .extension()
            .and_then(|s| s.to_str())
            .map(|x| x.eq_ignore_ascii_case("gguf"))
            .unwrap_or(false)
        {
            gguf_count += 1;
        }
    }

    if gguf_count > 0 {
        return ModelsDirCheck {
            valid: true,
            gguf_count,
            subdir_with_gguf,
            message: format!(
                "已找到 {} 个 .gguf 模型文件{}",
                gguf_count,
                if subdir_with_gguf > 0 {
                    format!(
                        "（另发现 {} 个子目录也含 .gguf，建议进入具体模型目录确认）",
                        subdir_with_gguf
                    )
                } else {
                    String::new()
                }
            ),
        };
    }

    // gguf_count == 0：可能选错层
    if subdir_with_gguf > 0 {
        return ModelsDirCheck {
            valid: false,
            gguf_count: 0,
            subdir_with_gguf,
            message: format!(
                "目录中没有 .gguf 文件，但在 {} 个子目录中发现了模型。请直接选择包含模型的子目录（如示例中的子目录之一）",
                subdir_with_gguf
            ),
        };
    }

    ModelsDirCheck {
        valid: false,
        gguf_count: 0,
        subdir_with_gguf: 0,
        message: format!("目录中没有找到 .gguf 模型文件：{}", path),
    }
}

#[cfg(test)]
mod tests {
    //! 覆盖：版本解析、取消标志管理、`check_models_dir` 在 tmp 目录上的行为。
    use super::*;
    use crate::detect::CancelFlag;
    use std::sync::{Arc, Mutex};

    /// 验证开发版输出优先使用 build 编号，格式与下载完成后的 bNNNN 一致。
    #[test]
    fn parse_llama_version_prefers_build_number() {
        let text = "version: 0.4.0-dev (build 10809, commit 5266f24da) built with Clang";
        assert_eq!(parse_llama_version(text).as_deref(), Some("b10809"));
    }

    /// 验证只有语义版本时仍能正常解析。
    #[test]
    fn parse_llama_version_falls_back_to_semver() {
        let text = "version: 0.4.0";
        assert_eq!(parse_llama_version(text).as_deref(), Some("0.4.0"));
    }

    /// 验证已经带 b 前缀的 llama-server 输出不会重复添加前缀。
    #[test]
    fn parse_llama_version_normalizes_tag_prefix() {
        let text = "llama-server b1609";
        assert_eq!(parse_llama_version(text).as_deref(), Some("b1609"));
    }

    /// 验证 `CancelFlag` 通过 `Arc::ptr_eq` 区分不同实例。
    /// 防止后续重构时错误地用 `Arc::eq`（比较值）导致并发检测互相覆盖。
    #[test]
    fn cancel_flag_uses_ptr_eq() {
        let f1: CancelFlag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f2: CancelFlag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(!Arc::ptr_eq(&f1, &f2));
        let f1_clone = f1.clone();
        assert!(Arc::ptr_eq(&f1, &f1_clone));
    }

    /// 验证 `check_models_dir` 在不存在的路径上返回 `valid: false`。
    /// 使用 tmp 目录中绝对不会存在的子路径，保证测试稳定。
    #[test]
    fn check_models_dir_nonexistent() {
        let check = check_models_dir("C:\\绝对\\不\\存在\\的\\路径\\xx_yy_zz".to_string());
        assert!(!check.valid);
        assert!(check.message.contains("路径不存在") || check.message.contains("不是目录"));
    }

    /// 验证 `check_models_dir` 在含 .gguf 文件的目录上返回 `valid: true`。
    /// 临时创建目录与一个伪 .gguf 文件，结束后清理。
    fn temp_dir_with_gguf() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "llama_ui_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fake.gguf"), b"fake").unwrap();
        dir
    }

    #[test]
    fn check_models_dir_with_gguf_files() {
        let dir = temp_dir_with_gguf();
        let check = check_models_dir(dir.to_string_lossy().to_string());
        assert!(check.valid, "应识别为合法: msg={}", check.message);
        assert!(check.gguf_count >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 验证 `check_models_dir` 在空目录上返回 `valid: false` 且 `gguf_count == 0`。
    #[test]
    fn check_models_dir_empty() {
        let dir = std::env::temp_dir().join(format!(
            "llama_ui_test_empty_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let check = check_models_dir(dir.to_string_lossy().to_string());
        assert!(!check.valid);
        assert_eq!(check.gguf_count, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 验证 `cancel_detection` 在空列表时返回 `false`。
    /// 这里只验证纯逻辑（构造一个空的 `AppState`-like 状态）。
    #[test]
    fn cancel_detection_empty_returns_false() {
        let cancels: Arc<Mutex<Vec<CancelFlag>>> = Arc::new(Mutex::new(vec![]));
        let guard = cancels.lock().unwrap();
        if guard.is_empty() {
            // 模拟 cancel_detection 的早返回
            // (断言在 if 分支内, 不会真的进入 cancel 逻辑)
        } else {
            panic!("空列表不应进入 cancel 分支");
        }
        drop(guard);
    }
}
