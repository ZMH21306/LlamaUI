//! 阶段 2：虚拟环境扫描（限 1.5s）。
//!
//! 在 home 下的常见项目根（projects / repos / dev / code / src / Documents
//! / Desktop）里找 `.venv` / `venv` / `env` / `llama-venv` / `.llama-venv`，
//! 命中其 `Scripts/llama-server.exe`（Windows）或 `bin/llama-server`（Unix）。
//!
//! 典型场景：开发者在 Python 虚拟环境里用 pip 装了 `llama-cpp-python`
//! 或从源码构建 llama.cpp 并把可执行文件放在 venv 里。
//!
//! # 取消 / 预算
//!
//! 每次 read_dir 之前调 `ctx.try_consume()` 扣入口预算，每轮循环
//! 头部 `ctx.check_deadline(ctx, 2)` 检查阶段 2 预算（1.5s）。
//! 命中立即返回；超时 / 取消 / 超额均返回 `None`。

use std::path::PathBuf;

use super::ctx::{Ctx, PER_DIR_LIMIT};

/// 阶段 2：在常见项目根下找 venv 里的 llama-server。
pub(crate) fn venv_scan(ctx: &Ctx) -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    let project_roots: Vec<PathBuf> = vec![
        home.join("projects"),
        home.join("repos"),
        home.join("dev"),
        home.join("code"),
        home.join("src"),
        home.join("Documents"),
        home.join("Desktop"),
    ];

    let venv_names: &[&str] = &[".venv", "venv", "env", "llama-venv", ".llama-venv"];

    for root in &project_roots {
        if ctx.check_deadline(2).is_err() {
            return None;
        }
        if !root.is_dir() {
            continue;
        }
        let entries: Vec<_> = match std::fs::read_dir(root) {
            Ok(r) => r.flatten().take(PER_DIR_LIMIT).collect(),
            Err(_) => continue,
        };
        for entry in entries {
            if !ctx.try_consume() {
                return None;
            }
            if ctx.is_cancelled() {
                return None;
            }
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            for vn in venv_names {
                let venv_dir = p.join(vn);
                if !venv_dir.is_dir() {
                    continue;
                }
                if let Some(cand) = venv_candidate(&venv_dir) {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// 给定 venv 根目录，返回 llama-server 在其中的可执行路径。
/// 平台分支：Windows → `Scripts\llama-server.exe`，Unix → `bin/llama-server`。
fn venv_candidate(venv_dir: &std::path::Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let cand = venv_dir.join("Scripts").join("llama-server.exe");
        if cand.is_file() {
            return Some(cand);
        }
    }
    #[cfg(not(windows))]
    {
        let cand = venv_dir.join("bin").join("llama-server");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
