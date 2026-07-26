//! 通用路径处理工具。
//!
//! 集中所有"路径比较 / 标准化 / 段匹配"逻辑。
//! 之前散落在 `detect.rs` 和 `server/port.rs` 中，重复实现且语义不完全一致。
//!
//! # 主要函数
//!
//! - [`normalize_for_compare`]：把路径转成"适合做大小写不敏感比较"的形式
//!   （小写、去末尾分隔符）。
//! - [`last_segment_eq`]：判断路径的**最后一段**是否等于某个值
//!   （例：`c:\foo\temp` → 末段 `temp`）。
//! - [`segment_eq_with_separator`]：判断路径中是否包含 `\<seg>\` 形式的子段
//!   （例：`c:\foo\temp\bar` 包含 `\temp\`）。
//! - [`is_world_writable_path`]：判断路径是否位于世界可写 / 临时目录
//!   （`tmp` / `temp` / `downloads`）。用于拒绝 PATH 注入。
//! - [`validate_executable_candidate`]：通用可执行文件白名单校验：
//!   必须是文件 + 文件名严格匹配 + 不在世界可写目录。

use std::path::{Path, PathBuf};

/// 把路径标准化为"适合大小写不敏感比较"的形式。
///
/// 1) 转 ASCII 小写
/// 2) 去掉末尾分隔符（`\` 或 `/`）
///
/// 不做 Unicode 规范化（NFC/NFD），因为 Windows 文件系统本身不区分大小写
/// 且路径组件比较时只按字面字节比较。
pub fn normalize_for_compare(p: &Path) -> String {
    let s = p.to_string_lossy().to_ascii_lowercase();
    let s = s.trim_end_matches(['\\', '/']);
    s.to_string()
}

/// 判断 `path` 的最后一段（路径组件）是否等于 `segment`。
///
/// 例：
/// - `c:\foo\temp` → 末段 `temp`
/// - `c:\foo\tempbar` → 末段 `tempbar`
/// - `c:\` → `""`（空段，永不匹配）
/// - `temp`（无分隔符） → 末段 `temp`
pub fn last_segment_eq(path: &str, segment: &str) -> bool {
    let p = path.trim_end_matches(['\\', '/']);
    match p.rsplit(['\\', '/']).next() {
        Some(last) => last == segment,
        None => false,
    }
}

/// 判断 `path` 中是否包含 `sep + segment + sep` 形式的子段。
///
/// 例：路径 `c:\foo\temp\bar` 在 Windows 上包含 `\temp\`（`sep = '\'`）。
/// 在 Unix 上路径 `c:/foo/temp/bar` 包含 `/temp/`（`sep = '/'`）。
pub fn segment_eq_with_separator(path: &str, sep: &str, segment: &str) -> bool {
    let needle = format!("{}{}{}", sep, segment, sep);
    path.contains(&needle)
}

/// 高风险目录段名。出现在父目录末段或中间段都视为可疑。
pub const RISKY_DIR_SEGMENTS: &[&str] = &["tmp", "temp", "downloads"];

/// 判断 `path` 是否位于世界可写 / 临时目录。
///
/// 检查两种情况（任一命中即返回 `true`）：
/// 1) 父目录的最后一段是 `tmp` / `temp` / `downloads`
/// 2) 父目录中包含 `\tmp\` / `\temp\` / `\downloads\`（Windows 风格）
///    或 `/tmp/` / `/temp/` / `/downloads/`（Unix 风格）
///
/// 用于拒绝 PATH 注入攻击（攻击者将恶意 `llama-server.exe` 放在 `c:\users\x\appdata\local\temp`
/// 并 prepend 到 PATH 中）。
pub fn is_world_writable_path(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let normalized = normalize_for_compare(parent);
    RISKY_DIR_SEGMENTS.iter().any(|seg| {
        last_segment_eq(&normalized, seg)
            || segment_eq_with_separator(&normalized, "\\", seg)
            || segment_eq_with_separator(&normalized, "/", seg)
    })
}

/// 通用可执行文件白名单校验。
///
/// 接受候选前做三项校验（任一不通过即返回 `None`）：
/// 1) 必须是 regular file（拒绝目录 / FIFO / device）
/// 2) 文件名必须严格等于 `allowed_names` 中的某一个
///    （按大小写不敏感比较，自动追加 `.exe` 变体）
/// 3) 父目录不能在世界可写位置（防 PATH 注入）
///
/// `allowed_names` 应只包含**文件名**（不含路径），如 `&["llama-server"]`。
pub fn validate_executable_candidate(p: &Path, allowed_names: &[&str]) -> Option<PathBuf> {
    if !p.is_file() {
        return None;
    }
    let name = p.file_name().and_then(|s| s.to_str())?.to_ascii_lowercase();
    let stem = name.trim_end_matches(".exe");
    if !allowed_names.iter().any(|allowed| {
        let allowed_lower = allowed.to_ascii_lowercase();
        let allowed_stem = allowed_lower.trim_end_matches(".exe");
        stem == allowed_stem
    }) {
        return None;
    }
    if is_world_writable_path(p) {
        return None;
    }
    Some(p.to_path_buf())
}

#[cfg(test)]
mod tests {
    //! 路径工具的纯函数测试。不依赖文件系统。
    use super::*;

    #[test]
    fn normalize_strips_trailing_separators() {
        let p = Path::new("C:\\Foo\\Bar\\");
        assert_eq!(normalize_for_compare(p), "c:\\foo\\bar");
    }

    #[test]
    fn normalize_handles_no_trailing_separator() {
        let p = Path::new("C:\\Foo\\Bar");
        assert_eq!(normalize_for_compare(p), "c:\\foo\\bar");
    }

    #[test]
    fn last_segment_finds_temp() {
        assert!(last_segment_eq("c:\\foo\\temp", "temp"));
    }

    #[test]
    fn last_segment_does_not_match_partial() {
        // `c:\foo\tempbar` 的末段是 `tempbar`，不是 `temp`。
        assert!(!last_segment_eq("c:\\foo\\tempbar", "temp"));
    }

    #[test]
    fn last_segment_handles_root() {
        // `c:\` 末段是空
        assert_eq!(last_segment_eq("c:\\", "temp"), false);
    }

    #[test]
    fn segment_eq_with_separator_finds_middle() {
        // 路径中段含 `\temp\`
        assert!(segment_eq_with_separator("c:\\foo\\temp\\bar", "\\", "temp"));
    }

    #[test]
    fn segment_eq_with_separator_rejects_partial() {
        // `tempbar` 不是 `temp` + 分隔符
        assert!(!segment_eq_with_separator("c:\\foo\\tempbar\\bar", "\\", "temp"));
    }

    #[test]
    fn is_world_writable_detects_temp_dir() {
        // 父目录末段是 temp
        assert!(is_world_writable_path(Path::new("C:\\users\\x\\AppData\\Local\\Temp\\llama-server.exe")));
    }

    #[test]
    fn is_world_writable_detects_middle_temp() {
        // 父目录中含 \temp\
        assert!(is_world_writable_path(Path::new("C:\\foo\\temp\\bar\\llama-server.exe")));
    }

    #[test]
    fn is_world_writable_allows_normal_dirs() {
        // 普通目录
        assert!(!is_world_writable_path(Path::new("C:\\Program Files\\llama.cpp\\llama-server.exe")));
    }

    #[test]
    fn is_world_writable_handles_relative_paths() {
        // 相对路径
        assert!(!is_world_writable_path(Path::new("target/debug/llama-server.exe")));
    }

    #[test]
    fn validate_candidate_rejects_directory() {
        let p = Path::new(".");
        // 当前目录不是 file
        assert!(validate_executable_candidate(p, &["llama-server"]).is_none());
    }

    #[test]
    fn validate_candidate_rejects_wrong_name() {
        // 不存在于测试环境中的文件，但 `is_file` 检查会先于名称检查
        let p = Path::new("C:\\nonexistent\\evil-llama.exe");
        assert!(validate_executable_candidate(p, &["llama-server"]).is_none());
    }
}
