//! 阶段 3：关键目录匹配（限 2.5s）。
//!
//! 在一组**静态白名单**的目录里找 llama-server / models：
//! - llama-server：`C:\Program Files\llama.cpp`、`%LOCALAPPDATA%\llama.cpp`、
//!   `~/scoop/apps/llama.cpp/current`、WinGet 包目录等
//! - models：`~/models`、`D:\models`、`%LOCALAPPDATA%\Models`、
//!   WinGet 包里的 `ggml.llamacpp_*/models` 等
//!
//! # 共享辅助
//!
//! - [`key_dir_roots`]：llama-server 候选根目录（OnceLock 缓存）
//! - [`find_file_in_dir`]：在目录（含 bin/Scripts/build/bin 子目录）里找可执行
//! - [`find_sibling_models_dir`]：已知 llama-server 路径后反推同级/父级 models
//! - [`find_any_llama_server`]：阶段 1-3 的快速路径找 llama-server（不扫全盘）
//! - [`has_gguf_top_level`]：判断目录直接子项里是否有 .gguf 文件
//!
//! # 安全
//!
//! 所有"扫描哪些目录"都走 [`key_dir_roots`] / 内置 `Vec` 的**静态白名单**，
//! 不会扫到用户任意目录；`find_file_in_dir` 也只查 `bin` / `Scripts` /
//! `build/bin` 三个预定义子目录名。

use std::path::{Path, PathBuf};

use super::ctx::{cached_path_bufs, Ctx, PER_DIR_LIMIT};

// ============================================================
// llama-server 关键目录匹配
// ============================================================

/// 阶段 3：白名单关键目录中找 `llama-server` 可执行。
pub(crate) fn key_dirs_llama(ctx: &Ctx) -> Option<PathBuf> {
    for root in key_dir_roots() {
        if ctx.check_deadline(3).is_err() {
            return None;
        }
        if !ctx.try_consume() {
            return None;
        }
        if !root.is_dir() {
            continue;
        }
        // 1) 直接子项
        if let Some(p) = find_file_in_dir(root, "llama-server", ctx) {
            return Some(p);
        }
        // 2) 浅递归一层：常见 build/bin 子目录
        if let Ok(rd) = std::fs::read_dir(root) {
            for entry in rd.flatten().take(PER_DIR_LIMIT) {
                if !ctx.try_consume() {
                    return None;
                }
                // DEFECT-005: 在 read_dir 循环中频繁检查 cancel，
                // 避免长 I/O 期间用户取消无响应。
                if ctx.is_cancelled() {
                    return None;
                }
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                if let Some(found) = find_file_in_dir(&p, "llama-server", ctx) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// 关键目录根列表（llama-server 候选路径）。
///
/// P2-6 修复：用 `OnceLock` 缓存单次进程内的计算结果。`dirs::*` 涉及
/// Windows SHGetKnownFolderPath + 解析 `%USERPROFILE%` 等系统调用，
/// 不应该每次检测都重做。
pub(crate) fn key_dir_roots() -> &'static [PathBuf] {
    cached_path_bufs(build_key_dir_roots)
}

fn build_key_dir_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if cfg!(windows) {
        // 系统盘与 Program Files
        roots.push(PathBuf::from("C:\\Program Files\\llama.cpp"));
        roots.push(PathBuf::from("C:\\Program Files (x86)\\llama.cpp"));
        roots.push(PathBuf::from("C:\\llama.cpp"));
        roots.push(PathBuf::from("D:\\llama.cpp"));
        roots.push(PathBuf::from("C:\\Program Files\\llama"));
        roots.push(PathBuf::from("C:\\llama"));
        roots.push(PathBuf::from("D:\\llama"));
        roots.push(PathBuf::from("C:\\llama.cpp\\build"));
        roots.push(PathBuf::from("C:\\llama.cpp\\build\\bin"));
        roots.push(PathBuf::from("D:\\llama.cpp\\build\\bin"));

        if let Some(local) = dirs::data_local_dir() {
            roots.push(local.join("Programs").join("llama.cpp"));
            roots.push(local.join("llama.cpp"));
            // WinGet 包：%LOCALAPPDATA%\Microsoft\WinGet\Packages
            // llama.cpp 的 WinGet 包会装出 ggml.llamacpp_<ver>\bin\llama-server.exe
            let winget = local.join("Microsoft").join("WinGet").join("Packages");
            roots.push(winget);
        }
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("llama.cpp"));
            roots.push(home.join("llama"));
            roots.push(home.join("llama.cpp").join("build").join("bin"));
            roots.push(home.join("Documents").join("llama.cpp"));
            roots.push(home.join("scoop").join("apps").join("llama.cpp"));
            roots.push(
                home.join("scoop")
                    .join("apps")
                    .join("llama.cpp")
                    .join("current"),
            );
        }
    } else {
        roots.push(PathBuf::from("/usr/local/bin"));
        roots.push(PathBuf::from("/usr/bin"));
        roots.push(PathBuf::from("/opt/llama.cpp"));
        roots.push(PathBuf::from("/opt/llama.cpp/bin"));
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join(".local").join("bin"));
            roots.push(home.join("llama.cpp"));
            roots.push(home.join("llama.cpp").join("build").join("bin"));
        }
    }
    roots
}

/// 在指定目录（含 `bin` / `Scripts` / `build/bin` 三个常见子目录）里找
/// `name.exe`（Windows）或 `name`（Unix）。命中即返回。
pub(crate) fn find_file_in_dir(dir: &Path, name: &str, _ctx: &Ctx) -> Option<PathBuf> {
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
    // 也查 build/bin/Scripts 子目录
    for sub in &["bin", "Scripts", "build/bin"] {
        let s = dir.join(sub);
        if !s.is_dir() {
            continue;
        }
        #[cfg(windows)]
        {
            let cand = s.join(format!("{}.exe", name));
            if cand.is_file() {
                return Some(cand);
            }
        }
        #[cfg(not(windows))]
        {
            let cand = s.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

// ============================================================
// 模型目录关键目录匹配
// ============================================================

/// 阶段 3 / 阶段 2：白名单关键目录中找含 `.gguf` 的目录。
///
/// 模型目录扫描不走 `key_dir_roots()`（那是 llama-server 专用），用一组
/// 独立的路径：home/models、Documents/models、各盘符根 models、
/// `%LOCALAPPDATA%\Models`、WinGet 包目录等。
pub(crate) fn key_dirs_models(ctx: &Ctx) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("models"));
        roots.push(home.join("Documents").join("models"));
        roots.push(home.join("Documents").join("Models"));
        roots.push(home.join("Desktop").join("models"));
        roots.push(home.join("Downloads").join("models"));
        roots.push(home.join("Downloads"));
    }
    if cfg!(windows) {
        for letter in ['C', 'D', 'E', 'F', 'G', 'H'] {
            roots.push(PathBuf::from(format!("{}:\\models", letter)));
        }
        if let Some(local) = dirs::data_local_dir() {
            roots.push(local.join("Models"));
            // WinGet: %LOCALAPPDATA%\Microsoft\WinGet\Packages\ggml.llamacpp_*\models
            roots.push(local.join("Microsoft").join("WinGet").join("Packages"));
            roots.push(local.join("llama.cpp").join("models"));
        }
        roots.push(PathBuf::from("C:\\Program Files\\llama.cpp\\models"));
        roots.push(PathBuf::from("C:\\ProgramData\\models"));
    } else {
        roots.push(PathBuf::from("/models"));
        roots.push(PathBuf::from("/mnt/models"));
        roots.push(PathBuf::from("/usr/local/share/llama.cpp/models"));
    }

    for root in &roots {
        if ctx.check_deadline(3).is_err() {
            return None;
        }
        if !ctx.try_consume() {
            return None;
        }
        if !root.is_dir() {
            continue;
        }
        // 直接子项是 .gguf
        if has_gguf_top_level(root) {
            return Some(root.clone());
        }
        // 浅递归一层找包含 .gguf 的子目录
        if let Ok(rd) = std::fs::read_dir(root) {
            for entry in rd.flatten().take(PER_DIR_LIMIT) {
                if !ctx.try_consume() {
                    return None;
                }
                // DEFECT-005: 在 read_dir 循环中频繁检查 cancel。
                if ctx.is_cancelled() {
                    return None;
                }
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                if has_gguf_top_level(&p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// 给定一个 `llama-server.exe` 路径，探查它**同级 / 父级**常见的 models 目录。
///
/// 这是修复「llama-server 目录下的 models 文件夹存在但没被识别」的关键。
/// 常见布局：
/// ```text
///   D:\llama.cpp\llama-server.exe
///   D:\llama.cpp\models\*.gguf        ← 同级 models
///   D:\llama.cpp\build\bin\llama-server.exe
///   D:\llama.cpp\build\bin\..\..\..\models  ← 父级 models
///   D:\tools\llama.cpp\llama-server.exe
///   D:\models\*.gguf                   ← 同盘符下的 models
/// ```
pub(crate) fn find_sibling_models_dir(llama_exe: &Path) -> Option<PathBuf> {
    let dir = llama_exe.parent()?;
    // 候选顺序：同级 → 父级 → 父级的父级 → 同盘符根 models
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1) 同级
    candidates.push(dir.to_path_buf());
    // 2) 父级（如 build/bin/ 下）
    if let Some(p) = dir.parent() {
        candidates.push(p.to_path_buf());
    }
    // 3) 父级的父级（罕见但兼容多层嵌套）
    if let Some(p) = dir.parent().and_then(|x| x.parent()) {
        candidates.push(p.to_path_buf());
    }
    // 4) 同盘符根 models（如 D:\models）
    if let Some(std::path::Component::Prefix(prefix)) = llama_exe.components().next() {
        for comp in prefix.as_os_str().to_string_lossy().chars() {
            if comp.is_ascii_alphabetic() {
                let root_models = PathBuf::from(format!("{}:\\models", comp));
                candidates.push(root_models);
                break;
            }
        }
    }
    // 简化版兜底：拿盘符
    if let Some(letter) = llama_exe.to_string_lossy().chars().next() {
        if letter.is_ascii_alphabetic() {
            let root_models = PathBuf::from(format!("{}:\\models", letter));
            if !candidates.contains(&root_models) {
                candidates.push(root_models);
            }
        }
    }

    for cand in &candidates {
        if !cand.is_dir() {
            continue;
        }
        // 1) 直接子项是 .gguf → 命中
        if has_gguf_top_level(cand) {
            return Some(cand.clone());
        }
        // 2) 浅找一层：cand 下有子目录包含 .gguf
        //
        // DEFECT-005 注：本函数无 ctx/cancel 访问（保持 helper 签名干净），
        // 且 candidates 上限 ≤ 5（同级 + 2 父级 + 盘符根 models），
        // 每次 read_dir 是单次调用而非长循环，cancel 响应压力低于其他位置。
        if let Ok(rd) = std::fs::read_dir(cand) {
            for entry in rd.flatten().take(PER_DIR_LIMIT) {
                let p = entry.path();
                if p.is_dir() && has_gguf_top_level(&p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// 用阶段 1-3 的快速路径找任意 `llama-server.exe` 路径（用于阶段 3 反推 models）。
/// 不会做全盘扫描；找不到就返回 `None`。
pub(crate) fn find_any_llama_server(ctx: &Ctx) -> Option<PathBuf> {
    if ctx.is_cancelled() || ctx.is_timed_out() {
        return None;
    }
    if let Ok(p) = std::env::var("LLAMA_SERVER_PATH") {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Ok(p) = which::which("llama-server") {
        return Some(p);
    }
    for root in key_dir_roots() {
        if ctx.is_cancelled() || ctx.is_timed_out() {
            return None;
        }
        if !ctx.try_consume() {
            return None;
        }
        if !root.is_dir() {
            continue;
        }
        if let Some(p) = find_file_in_dir(root, "llama-server", ctx) {
            return Some(p);
        }
        if let Ok(rd) = std::fs::read_dir(root) {
            for entry in rd.flatten().take(PER_DIR_LIMIT) {
                if !ctx.try_consume() {
                    return None;
                }
                if ctx.is_cancelled() {
                    return None;
                }
                let p = entry.path();
                if p.is_dir() {
                    if let Some(found) = find_file_in_dir(&p, "llama-server", ctx) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

/// 判断目录直接子项里是否有 `.gguf` 文件。
///
/// 限制 readdir 数量：单目录大于 `MAX_SCAN`（多为非 gguf 的素材库）就
/// 提前判定 false，避免在含几万文件的目录上卡死。
pub(crate) fn has_gguf_top_level(dir: &Path) -> bool {
    const MAX_SCAN: usize = 200;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for (i, e) in rd.flatten().enumerate() {
            if i >= MAX_SCAN {
                return false;
            }
            let p = e.path();
            if p.is_file()
                && p.extension()
                    .and_then(|s| s.to_str())
                    .map(|x| x.eq_ignore_ascii_case("gguf"))
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    //! 关键目录根列表 + 缓存 + `has_gguf_top_level` 测试。
    //!
    //! `find_file_in_dir` / `find_sibling_models_dir` / `find_any_llama_server`
    //! 都需要 `Ctx` 走 `try_consume` / `check_deadline`，而本项目未启用
    //! `tauri/test` 特性，单元测试无法构造 `AppHandle`。这些函数的
    //! 端到端正确性通过 `mod.rs::tests::detect_loop_responds_to_cancel` 与
    //! 前端集成测试间接覆盖。
    use super::*;

    /// 验证 `key_dir_roots` 缓存只算一次。
    #[test]
    fn key_dir_roots_caches_result() {
        let r1: &[PathBuf] = key_dir_roots();
        let r2: &[PathBuf] = key_dir_roots();
        assert_eq!(r1.as_ptr(), r2.as_ptr(), "OnceLock 缓存必须返回相同指针");
    }

    /// 验证 `has_gguf_top_level` 在不存在的目录上返回 false。
    #[test]
    fn has_gguf_top_level_nonexistent_returns_false() {
        let p = std::env::current_dir()
            .expect("cwd")
            .join("definitely-not-here-gguf-test");
        assert!(!has_gguf_top_level(&p));
    }

    /// 验证 `has_gguf_top_level` 在含 .gguf 的临时目录上返回 true。
    #[test]
    fn has_gguf_top_level_with_gguf_returns_true() {
        let dir = std::env::temp_dir().join(format!(
            "llama_ui_gguf_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("fake.gguf"), b"x").expect("write");
        let r = has_gguf_top_level(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r, "含 .gguf 的目录应返回 true");
    }

    /// 验证 `has_gguf_top_level` 在空目录上返回 false。
    #[test]
    fn has_gguf_top_level_empty_returns_false() {
        let dir = std::env::temp_dir().join(format!(
            "llama_ui_empty_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let r = has_gguf_top_level(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!r, "空目录应返回 false");
    }
}
