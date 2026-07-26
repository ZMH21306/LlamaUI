//! 启动初始化的步骤 ③：自动加载配置。
//!
//! 探测 llama-server 路径与模型目录，完成参数初始化摘要。

use std::path::Path;
use tauri::AppHandle;

use crate::config::AppConfig;
use crate::log::emit_log_to;

use super::STEP_INIT;

/// ③ 自动加载配置：探测 llama 路径、模型目录
pub(super) fn step_auto_load(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    emit_log_to(app, "system", "开始自动加载配置", Some(STEP_INIT));

    // 解析最终生效的 llama 路径
    let resolved = resolve_program(cfg);
    if let Some(p) = &cfg.llama_server_path {
        if !p.is_empty() {
            emit_log_to(
                app,
                "system",
                &format!("使用自定义 llama-server 路径：{}", p),
                Some(STEP_INIT),
            );
        }
    }
    match which::which(&resolved) {
        Ok(p) => emit_log_to(
            app,
            "system",
            &format!("最终 llama-server 路径：{}", p.display()),
            Some(STEP_INIT),
        ),
        Err(_) => emit_log_to(
            app,
            "system",
            &format!("PATH 中未找到 {}，将按原样尝试启动", resolved),
            Some(STEP_INIT),
        ),
    }

    // 模型目录探测
    if cfg.models_dir.trim().is_empty() {
        emit_log_to(
            app,
            "system",
            "模型目录未配置，尝试在常见位置自动定位...",
            Some(STEP_INIT),
        );
        let candidates = default_model_dirs();
        for dir in &candidates {
            if dir.is_empty() {
                continue;
            }
            let p = Path::new(dir);
            if !p.exists() || !p.is_dir() {
                continue;
            }
            if has_gguf(p) {
                emit_log_to(
                    app,
                    "system",
                    &format!("自动定位到模型目录：{}", dir),
                    Some(STEP_INIT),
                );
                emit_log_to(
                    app,
                    "system",
                    "（已在前端提示，请在左侧确认后保存）",
                    Some(STEP_INIT),
                );
                return Ok(());
            }
        }
        emit_log_to(
            app,
            "system",
            "未在常见位置找到含 .gguf 的目录，请手动在左侧设置",
            Some(STEP_INIT),
        );
    } else {
        emit_log_to(
            app,
            "system",
            &format!("使用已配置的模型目录：{}", cfg.models_dir),
            Some(STEP_INIT),
        );
    }

    // 参数初始化摘要
    emit_log_to(
        app,
        "system",
        &format!("上下文长度：{}", cfg.ctx_size),
        Some(STEP_INIT),
    );
    emit_log_to(
        app,
        "system",
        &format!("GPU 卸载层数：{}", cfg.n_gpu_layers),
        Some(STEP_INIT),
    );
    emit_log_to(
        app,
        "system",
        &format!("监听端口：{}（自动切换：{}）", cfg.port, cfg.auto_port),
        Some(STEP_INIT),
    );

    emit_log_to(app, "system", "参数初始化完成", Some(STEP_INIT));
    Ok(())
}

/// 解析可执行文件路径（与 server::cmdline::resolve_program 保持一致）。
pub(crate) fn resolve_program(cfg: &AppConfig) -> String {
    if let Some(custom) = &cfg.llama_server_path {
        if !custom.is_empty() {
            return custom.clone();
        }
    }
    "llama-server".to_string()
}

/// 常见模型目录候选
fn default_model_dirs() -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(home) = std::env::var("USERPROFILE") {
            dirs.push(format!("{}\\models", home));
        }
        dirs.push("C:\\models".into());
        dirs.push("D:\\models".into());
        dirs.push("C:\\llama\\models".into());
        dirs.push("D:\\llama\\models".into());
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(format!("{}/models", home));
            dirs.push(format!("{}/.cache/llama.cpp", home));
        }
        dirs.push("/opt/llama/models".into());
        dirs.push("/usr/local/share/llama.cpp/models".into());
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(format!("{}/models", home));
            dirs.push(format!("{}/.cache/llama.cpp", home));
        }
        dirs.push("/opt/llama.cpp/models".into());
        dirs.push("/usr/local/share/llama.cpp/models".into());
    }
    dirs
}

/// 判断目录中是否含 .gguf 文件
fn has_gguf(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("gguf"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
