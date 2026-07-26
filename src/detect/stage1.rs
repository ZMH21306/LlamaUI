//! 阶段 1：环境变量 + PATH（< 100ms）。
//!
//! 这是最便宜的检测路径，绝大多数命中都在这里完成：
//! - 显式环境变量 `LLAMA_SERVER_PATH` / `LLAMA_MODELS_DIR`（用户/部署方主动设置）
//! - `which::which("llama-server")` 走系统 PATH
//!
//! # 安全
//!
//! P2-1 修复：把 `which::which` 的返回路径用
//! [`crate::util::path::validate_executable_candidate`] 做白名单校验，
//! 拒绝：
//! 1) 父目录在 `/tmp` 等世界可写位置（防 PATH 注入攻击）
//! 2) 文件名不严格等于 `llama-server` / `llama-server.exe`（防诱饵命名）
//! 3) 非 regular file（防目录 / FIFO / device）

use std::path::{Path, PathBuf};

use crate::util::path::validate_executable_candidate;

/// 阶段 1：解析 `llama-server.exe` 可执行路径（环境变量 → PATH → None）。
///
/// 命中时返回通过白名单校验的绝对路径；未命中返回 `None`。
///
/// 校验在 `which::which` 之后做（而不是之前），理由是 `which` 自身
/// 已经按系统 PATH 顺序找过了，我们只需要在「找到的候选」上做最后
/// 一层防御。
pub(crate) fn llama() -> Option<PathBuf> {
    const ALLOWED: &[&str] = &["llama-server", "llama-server.exe"];

    if let Ok(p) = std::env::var("LLAMA_SERVER_PATH") {
        if let Some(pb) = validate_executable_candidate(Path::new(&p), ALLOWED) {
            return Some(pb);
        }
    }
    if let Ok(p) = which::which("llama-server") {
        if let Some(pb) = validate_executable_candidate(&p, ALLOWED) {
            return Some(pb);
        }
    }
    None
}

/// 阶段 1：解析 `models_dir`（仅环境变量）。
///
/// 不走 PATH 也不做白名单：模型目录允许任意用户目录（home / Download / 任意盘符），
/// 不需要做"可执行白名单"那种强约束。唯一约束是「必须存在且是目录」，
/// 这是「读」类操作，不会被 RCE 利用。
pub(crate) fn models() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LLAMA_MODELS_DIR") {
        let pb = PathBuf::from(&p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    None
}

/// P2-1 白名单校验辅助函数（保留为阶段 1 的语义包装，便于未来扩展）。
///
/// 实现委托给 `util::path::validate_executable_candidate`，本函数的存在是为了
/// 兼容历史调用点与测试代码。如需调整白名单规则，请直接修改 `util::path`。
#[allow(dead_code)]
pub(crate) fn validate_llama_candidate(p: &Path) -> Option<PathBuf> {
    validate_executable_candidate(p, &["llama-server", "llama-server.exe"])
}

#[cfg(test)]
mod tests {
    //! 白名单校验回归测试（P2-1）。
    //!
    //! 覆盖：
    //! - 合法的 `llama-server.exe` 通过
    //! - 诱饵文件名（带后缀、含 camelCase）被拒绝
    //! - `/tmp` 等世界可写目录下的"合法名"被拒绝
    //! - 目录、FIFO 等非 regular file 被拒绝
    use super::*;

    /// 合法的 `llama-server.exe` 通过白名单
    #[cfg(windows)]
    #[test]
    fn validate_accepts_named_llama_server_exe() {
        // 在 cwd 下放一个 llama-server.exe，验证 happy path
        let cwd_bin = std::env::current_dir()
            .expect("cwd")
            .join("llama-server.exe");
        std::fs::write(&cwd_bin, b"fake exe").expect("write cwd");
        let result = validate_llama_candidate(&cwd_bin);
        let _ = std::fs::remove_file(&cwd_bin);
        assert!(result.is_some(), "cwd 下的合法文件名应通过：{:?}", result);
    }

    /// 父目录在 `/tmp` 等高风险位置时拒绝（即使文件名正确）
    #[test]
    fn validate_rejects_tmp_dir_parent() {
        let tmp = std::env::temp_dir().join("llama-server.exe");
        std::fs::write(&tmp, b"fake exe").expect("write tmp");
        let result = validate_llama_candidate(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert!(
            result.is_none(),
            "高风险父目录下的合法文件名应被拒绝：{:?}",
            result
        );
    }

    /// 诱饵文件名（带数字/下划线/不同 stem）被拒绝
    ///
    /// 注意：不在列表中加 `Llama-Server.exe`（Windows NTFS 大小写不敏感，
    /// 它就是同一个文件，accept 才是正确的）。
    #[test]
    fn validate_rejects_lookalike_names() {
        let cwd = std::env::current_dir().expect("cwd");
        for name in [
            "llama-server2.exe",
            "llama_server.exe",
            "llamacpp.exe",
            "evil-llama-server.exe",
        ] {
            let p = cwd.join(name);
            std::fs::write(&p, b"x").expect("write");
            let r = validate_llama_candidate(&p);
            let _ = std::fs::remove_file(&p);
            assert!(r.is_none(), "诱饵 `{}` 必须被拒绝：{:?}", name, r);
        }
    }

    /// 不存在的文件被拒绝
    #[test]
    fn validate_rejects_nonexistent() {
        let p = std::env::current_dir()
            .expect("cwd")
            .join("definitely-not-here-llama-server.exe");
        assert!(validate_llama_candidate(&p).is_none());
    }

    /// 目录不是 regular file，被拒绝
    #[test]
    fn validate_rejects_directory() {
        let p = std::env::current_dir()
            .expect("cwd")
            .join("llama-server.exe.but_actually_a_dir");
        std::fs::create_dir_all(&p).expect("mkdir");
        let r = validate_llama_candidate(&p);
        let _ = std::fs::remove_dir(&p);
        assert!(r.is_none(), "目录必须被拒绝：{:?}", r);
    }
}
