//! 阶段 4：全盘深度扫描（兜底，限 5s 累计）。
//!
//! 当阶段 1-3 全部未命中时，对每个盘符根做有限深度的递归扫描：
//! - llama-server：在每个盘符根下递归找 `llama-server.exe`
//! - models：在每个盘符根下递归找含 `.gguf` 的目录
//!
//! # 资源保护
//!
//! - 时间预算：阶段 4 累计 ≤ 5s（`STAGE_BUDGET_4_MS`），全阶段累计 ≤ 10s
//! - 入口预算：单次检测访问条目数 ≤ 30k（`MAX_ENTRIES`）
//! - 递归深度：`FULL_DISK_DEPTH = 4`（防止无限递归）
//! - 黑名单目录：`SKIP_NAMES`（node_modules / Windows / 回收站 / proc / sys 等）
//! - 隐藏目录：`.` 开头的目录名一律跳过
//! - 周期性 emit：每 800 节点一次 `detect-progress` 事件，避免 UI 卡死错觉
//!
//! # 取消
//!
//! 每次 read_dir 之前 / 之内都会检查 `ctx.is_cancelled()`，调用方点击
//! 「取消」后下一个 I/O 边界即返回。

use std::path::{Path, PathBuf};

use super::ctx::{cached_path_bufs, Ctx, TOTAL_BUDGET_MS};
use super::stage3::has_gguf_top_level;

/// 全盘扫描的递归深度上限
const FULL_DISK_DEPTH: usize = 4;

/// 跳过的目录名（黑名单）
const SKIP_NAMES: &[&str] = &[
    ".",
    "..",
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    "$recycle.bin",
    "system volume information",
    "recovery",
    "perflogs",
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users", // C:\Users 太深；home 单独走 drive_roots()
    "proc",
    "sys",
    "dev",
    "boot",
    "snap",
    "var",
    "tmp",
    "cache",
    "appdata",
    "application data",
];

/// 阶段 4：在所有盘符根下递归找 `llama-server.exe`。
pub(crate) fn full_disk_llama(ctx: &Ctx) -> Option<PathBuf> {
    let roots = drive_roots();
    for (i, root) in roots.iter().enumerate() {
        if ctx.check_deadline(4).is_err() {
            return None;
        }
        // 阶段 4 进度反馈：每开始一个盘符就 emit 一次，避免 UI 看着像卡死
        ctx.emit(
            4,
            "④ 全盘深度扫描（兜底）",
            &format!("扫描盘符 {}/{}：{}", i + 1, roots.len(), root.display()),
            false,
            "running",
        );
        if let Some(found) = find_exe_recursive(root, FULL_DISK_DEPTH, "llama-server", ctx) {
            return Some(found);
        }
    }
    None
}

/// 阶段 4：在所有盘符根下递归找含 `.gguf` 的目录。
pub(crate) fn full_disk_models(ctx: &Ctx) -> Option<PathBuf> {
    let roots = drive_roots();
    for (i, root) in roots.iter().enumerate() {
        if ctx.check_deadline(4).is_err() {
            return None;
        }
        // 阶段 4 进度反馈
        ctx.emit(
            4,
            "④ 全盘深度扫描（兜底）",
            &format!("扫描盘符 {}/{}：{}", i + 1, roots.len(), root.display()),
            false,
            "running",
        );
        if let Some(found) = find_gguf_dir_recursive(root, FULL_DISK_DEPTH, ctx) {
            return Some(found);
        }
    }
    None
}

/// 盘符根列表（Windows 枚举 `C:\..Z:\`，非 Windows 取 `/` + home）。
///
/// 用 `OnceLock` 缓存：每次检测都重新枚举 'C'..='Z' 盘符（24 次 sysinfo +
/// 24 次 Path::is_dir）是浪费，缓存后只在首次调用时计算。
pub(crate) fn drive_roots() -> &'static [PathBuf] {
    cached_path_bufs(build_drive_roots)
}

fn build_drive_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        for letter in 'C'..='Z' {
            let p = PathBuf::from(format!("{}:\\", letter));
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    #[cfg(not(windows))]
    {
        roots.push(PathBuf::from("/"));
        if let Some(home) = dirs::home_dir() {
            roots.push(home);
        }
    }
    roots
}

/// 递归在 `root` 下找 `name` 可执行文件。`max_depth` 控制递归深度。
fn find_exe_recursive(root: &Path, max_depth: usize, name: &str, ctx: &Ctx) -> Option<PathBuf> {
    fn rec(dir: &Path, depth: usize, max: usize, name: &str, ctx: &Ctx) -> Option<PathBuf> {
        // 频繁检查取消 / 超时：避免被长时间阻塞
        if ctx.is_cancelled() {
            return None;
        }
        if ctx.elapsed() > TOTAL_BUDGET_MS {
            return None;
        }
        if !ctx.try_consume() {
            return None;
        }
        // 周期性 emit：每 800 节点一次，避免 UI 看着像卡死
        if ctx.entries() % 800 == 0 && ctx.entries() > 0 {
            ctx.emit(
                4,
                "④ 全盘深度扫描（兜底）",
                &format!("已扫描 {} 项…", ctx.entries()),
                false,
                "running",
            );
        }
        if !dir.is_dir() {
            return None;
        }

        // 当前目录命中？
        #[cfg(windows)]
        {
            let cand = dir.join(format!("{}.exe", name));
            if cand.is_file() {
                return Some(cand);
            }
        }
        #[cfg(not(windows))]
        {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }

        if depth >= max {
            return None;
        }

        let rd = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return None,
        };

        let mut entries: Vec<_> = rd
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| !should_skip_dir(n))
                    .unwrap_or(false)
            })
            .take(super::ctx::PER_DIR_LIMIT)
            .collect();
        entries.sort_by_key(|e| e.path());

        for e in entries {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            if let Some(found) = rec(&p, depth + 1, max, name, ctx) {
                return Some(found);
            }
        }
        None
    }
    rec(root, 0, max_depth, name, ctx)
}

/// 递归在 `root` 下找含 `.gguf` 的目录。
fn find_gguf_dir_recursive(root: &Path, max_depth: usize, ctx: &Ctx) -> Option<PathBuf> {
    fn rec(dir: &Path, depth: usize, max: usize, ctx: &Ctx) -> Option<PathBuf> {
        if ctx.is_cancelled() || ctx.elapsed() > TOTAL_BUDGET_MS {
            return None;
        }
        if !ctx.try_consume() {
            return None;
        }
        // 周期性 emit：每 800 节点一次
        if ctx.entries() % 800 == 0 && ctx.entries() > 0 {
            ctx.emit(
                4,
                "④ 全盘深度扫描（兜底）",
                &format!("已扫描 {} 项…", ctx.entries()),
                false,
                "running",
            );
        }
        if !dir.is_dir() {
            return None;
        }
        if has_gguf_top_level(dir) {
            return Some(dir.to_path_buf());
        }
        if depth >= max {
            return None;
        }
        let rd = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return None,
        };
        let mut entries: Vec<_> = rd
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| !should_skip_dir(n))
                    .unwrap_or(false)
            })
            .take(super::ctx::PER_DIR_LIMIT)
            .collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            if let Some(found) = rec(&p, depth + 1, max, ctx) {
                return Some(found);
            }
        }
        None
    }
    rec(root, 0, max_depth, ctx)
}

/// 是否应跳过该目录名（黑名单匹配 + 隐藏目录）。
pub(crate) fn should_skip_dir(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.starts_with('.') {
        return true;
    }
    SKIP_NAMES.iter().any(|s| *s == n)
}

#[cfg(test)]
mod tests {
    //! 纯函数 + OnceLock 缓存测试。`find_exe_recursive` /
    //! `find_gguf_dir_recursive` 需要 `Ctx` + `AppHandle`，无法单测。
    use super::*;

    /// 验证 `drive_roots` 缓存只算一次。
    #[test]
    fn drive_roots_caches_result() {
        let r1: &[PathBuf] = drive_roots();
        let r2: &[PathBuf] = drive_roots();
        assert_eq!(r1.as_ptr(), r2.as_ptr(), "OnceLock 缓存必须返回相同指针");
    }

    /// `should_skip_dir`：黑名单目录被跳过
    #[test]
    fn should_skip_dir_blacklist() {
        for name in [
            "node_modules",
            ".git",
            "Windows",
            "Program Files",
            "$Recycle.Bin",
            "ProgramData",
        ] {
            assert!(should_skip_dir(name), "`{}` 应被跳过", name);
        }
    }

    /// `should_skip_dir`：隐藏目录（`.` 开头）一律跳过
    #[test]
    fn should_skip_dir_dot_prefix() {
        for name in [".cache", ".config", ".local", ".venv"] {
            assert!(should_skip_dir(name), "`{}` 应被跳过", name);
        }
    }

    /// `should_skip_dir`：合法目录名不跳过
    #[test]
    fn should_skip_dir_keeps_normal() {
        for name in ["models", "llama.cpp", "downloads", "projects", "repos"] {
            assert!(!should_skip_dir(name), "`{}` 应保留", name);
        }
    }

    /// `SKIP_NAMES` 必须含「users」（C:\Users 太深）。
    /// 这是性能关键约束：漏了会扫到 100k+ 文件。
    #[test]
    fn skip_names_includes_users() {
        assert!(SKIP_NAMES.contains(&"users"));
    }
}
