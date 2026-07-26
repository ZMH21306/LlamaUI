//! 启动初始化的步骤 ①：环境检查。
//!
//! 检查项：操作系统、llama-server 可用性、模型目录合法性。
//! 任何一项失败都会立即返回错误，前端会在 UI 中显示具体原因。

use std::path::Path;
use tauri::AppHandle;

use crate::config::AppConfig;
use crate::log::emit_log_to;

use super::STEP_ENV;

/// ① 环境检查
pub(super) fn step_env_check(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    emit_log_to(app, "system", "开始环境检查", Some(STEP_ENV));

    // 当前操作系统
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    emit_log_to(
        app,
        "system",
        &format!("检测到操作系统：{} ({})", os, arch),
        Some(STEP_ENV),
    );

    // 工作目录 / Tauri 资源目录（仅展示，便于排错）
    if let Ok(cwd) = std::env::current_dir() {
        emit_log_to(
            app,
            "system",
            &format!("当前工作目录：{}", cwd.display()),
            Some(STEP_ENV),
        );
    }

    // llama-server 可用性
    let program = super::auto_load::resolve_program(cfg);
    if let Some(p) = &cfg.llama_server_path {
        if !p.is_empty() {
            let path = Path::new(p);
            if path.exists() {
                emit_log_to(
                    app,
                    "system",
                    &format!("使用自定义 llama-server 路径：{}", p),
                    Some(STEP_ENV),
                );
            } else {
                emit_log_to(
                    app,
                    "system",
                    &format!("警告：自定义路径不存在：{}", p),
                    Some(STEP_ENV),
                );
            }
        }
    }
    match which::which(&program) {
        Ok(p) => emit_log_to(
            app,
            "system",
            &format!("在 PATH 中找到：{}", p.display()),
            Some(STEP_ENV),
        ),
        Err(_) => emit_log_to(
            app,
            "system",
            &format!("未在 PATH 中找到 {}（将在 ② 步处理）", program),
            Some(STEP_ENV),
        ),
    }

    // 模型目录
    if cfg.models_dir.trim().is_empty() {
        emit_log_to(
            app,
            "system",
            "模型目录未配置，将在 ③ 步尝试自动定位",
            Some(STEP_ENV),
        );
    } else {
        let p = Path::new(&cfg.models_dir);
        if !p.exists() {
            let msg = format!("模型目录不存在：{}", cfg.models_dir);
            emit_log_to(app, "system", &msg, Some(STEP_ENV));
            return Err(msg);
        }
        if !p.is_dir() {
            let msg = format!("路径不是目录：{}", cfg.models_dir);
            emit_log_to(app, "system", &msg, Some(STEP_ENV));
            return Err(msg);
        }

        // 统计 .gguf 文件数量
        let mut gguf_count = 0usize;
        let mut names: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_gguf = path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("gguf"))
                    .unwrap_or(false);
                if is_gguf {
                    gguf_count += 1;
                    if names.len() < 10 {
                        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                            names.push(name.to_string());
                        }
                    }
                }
            }
        }
        emit_log_to(
            app,
            "system",
            &format!("模型目录合法，共 {} 个 .gguf 文件", gguf_count),
            Some(STEP_ENV),
        );
        for n in &names {
            emit_log_to(app, "system", &format!("  • {}", n), Some(STEP_ENV));
        }
        if gguf_count == 0 {
            let msg = "模型目录下未发现 .gguf 文件，请将模型放入该目录".to_string();
            emit_log_to(app, "system", &msg, Some(STEP_ENV));
            return Err(msg);
        }
    }

    // 端口范围
    if cfg.port == 0 {
        let msg = "端口不能为 0".to_string();
        emit_log_to(app, "system", &msg, Some(STEP_ENV));
        return Err(msg);
    }

    emit_log_to(app, "system", "环境检查通过", Some(STEP_ENV));
    Ok(())
}
