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

/// P0-2 安全修复：对下载/保存类输入的文件名做严格净化，拒绝路径遍历攻击。
///
/// 背景：HuggingFace 模型文件名由 HF API 返回（`siblings[].rfilename`），理论上是
/// 相对路径（如 `model.gguf` 或 `subdir/model.gguf`）。但若 API 被劫持或前端
/// 拼接时混入恶意字符，攻击者可构造 `../etc/passwd` 或 `..\evil.exe` 写入
/// 指定目录之外，造成任意文件覆盖 / 任意文件写入。
///
/// 规则：
/// - 拒绝含 `..` 段的路径（`..` / `../` / `..\` / `foo/../../bar`）
/// - 拒绝绝对路径（`/` 开头、`X:\` 开头、`\\` UNC）
/// - 拒绝 Windows 盘符前缀（`C:` / `c:`）
/// - 拒绝 NUL 字符
/// - 拒绝 Windows 设备名（`CON` / `NUL` / `PRN` / `AUX` / `LPT1` 等）
/// - 拒绝控制字符（`< 0x20`）
/// - 拒绝长度 > 255 的单段
/// - 拒绝多段路径（仅允许单层文件名，如 `model.gguf`）
///
/// 返回净化后的**最后一段**文件名（剥掉目录前缀，仅保留 `basename`），
/// 调用方应将其与目标目录拼接后保存。
///
/// # Examples
/// ```rust
/// use llama_ui_lib::util::path::sanitize_filename;
/// assert_eq!(sanitize_filename("model.gguf"), Ok("model.gguf".to_string()));
/// assert_eq!(sanitize_filename("../etc/passwd").as_ref().err().map(|e| e.to_string()), Some("路径遍历（含 `..` 段）".to_string()));
/// ```
pub fn sanitize_filename(input: &str) -> Result<String, FilenameError> {
    if input.is_empty() {
        return Err(FilenameError::Empty);
    }
    if input.contains('\0') {
        return Err(FilenameError::Nul);
    }
    // 拒绝控制字符（含 \r \n \t \x00-\x1F）
    if input.bytes().any(|b| b < 0x20) {
        return Err(FilenameError::ControlChar);
    }
    // 拒绝绝对路径：Unix `/` 开头、UNC `\\` 开头、Windows 盘符 `X:\` 开头
    if input.starts_with('/')
        || input.starts_with('\\')
        || input.len() >= 2
            && input.as_bytes()[1] == b':'
            && (input.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(FilenameError::AbsolutePath);
    }
    // 拒绝多段路径（含 `/` 或 `\` 且不止一段）
    let has_sep = input.contains('/') || input.contains('\\');
    if has_sep {
        // 逐段检查 `..` / `.` / 设备名 / 长度
        for seg in input.split(['/', '\\']) {
            if seg.is_empty() {
                // 允许尾部的空段（如 `model.gguf/` 会被 trim 处理）
                continue;
            }
            if seg == "." || seg == ".." {
                return Err(FilenameError::PathTraversal);
            }
            if seg.len() > 255 {
                return Err(FilenameError::SegmentTooLong);
            }
            let seg_upper = seg.to_ascii_uppercase();
            if matches!(
                seg_upper.as_str(),
                "CON" | "NUL" | "PRN" | "AUX"
                    | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
                    | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            ) {
                return Err(FilenameError::DeviceName);
            }
        }
        // 多段路径：拒绝（仅允许单层文件名）
        return Err(FilenameError::Subdirectory);
    }
    // 单段路径：直接做设备名 / 长度检查
    if input.len() > 255 {
        return Err(FilenameError::SegmentTooLong);
    }
    let upper = input.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "NUL" | "PRN" | "AUX"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
    ) {
        return Err(FilenameError::DeviceName);
    }
    Ok(input.to_string())
}

/// 文件名净化失败的错误类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilenameError {
    Empty,
    Nul,
    ControlChar,
    AbsolutePath,
    PathTraversal,
    SegmentTooLong,
    DeviceName,
    Subdirectory,
}

impl std::fmt::Display for FilenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("文件名不能为空"),
            Self::Nul => f.write_str("文件名含 NUL 字符"),
            Self::ControlChar => f.write_str("文件名含控制字符"),
            Self::AbsolutePath => f.write_str("文件名不能是绝对路径"),
            Self::PathTraversal => f.write_str("路径遍历（含 `..` 段）"),
            Self::SegmentTooLong => f.write_str("文件名单段超过 255 字节"),
            Self::DeviceName => f.write_str("文件名是 Windows 设备名（CON/NUL/PRN/AUX/LPT*/COM*）"),
            Self::Subdirectory => f.write_str("文件名不能含子目录（仅允许单层文件名）"),
        }
    }
}

impl std::error::Error for FilenameError {}

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

    // ---- P0-2 sanitize_filename 测试 ----

    #[test]
    fn sanitize_filename_accepts_simple() {
        assert_eq!(sanitize_filename("model.gguf").unwrap(), "model.gguf");
    }

    #[test]
    fn sanitize_filename_rejects_path_traversal() {
        assert_eq!(sanitize_filename("../etc/passwd").err(), Some(FilenameError::PathTraversal));
        assert_eq!(sanitize_filename("..\\evil.exe").err(), Some(FilenameError::PathTraversal));
        assert_eq!(sanitize_filename("foo/../../bar").err(), Some(FilenameError::PathTraversal));
    }

    #[test]
    fn sanitize_filename_rejects_absolute() {
        assert_eq!(sanitize_filename("/etc/passwd").err(), Some(FilenameError::AbsolutePath));
        assert_eq!(sanitize_filename("\\\\server\\share\\file").err(), Some(FilenameError::AbsolutePath));
    }

    #[test]
    fn sanitize_filename_rejects_windows_drive() {
        assert_eq!(sanitize_filename("C:\\windows\\system32\\cmd.exe").err(), Some(FilenameError::AbsolutePath));
        assert_eq!(sanitize_filename("d:/data/model.gguf").err(), Some(FilenameError::AbsolutePath));
    }

    #[test]
    fn sanitize_filename_rejects_device_names() {
        assert_eq!(sanitize_filename("CON").err(), Some(FilenameError::DeviceName));
        assert_eq!(sanitize_filename("NUL").err(), Some(FilenameError::DeviceName));
        assert_eq!(sanitize_filename("LPT1").err(), Some(FilenameError::DeviceName));
        assert_eq!(sanitize_filename("COM1").err(), Some(FilenameError::DeviceName));
        assert_eq!(sanitize_filename("prn").err(), Some(FilenameError::DeviceName)); // 大小不敏感
    }

    #[test]
    fn sanitize_filename_rejects_control_chars() {
        assert_eq!(sanitize_filename("model\x00.gguf").err(), Some(FilenameError::Nul));
        assert_eq!(sanitize_filename("model\n.gguf").err(), Some(FilenameError::ControlChar));
    }

    #[test]
    fn sanitize_filename_rejects_empty() {
        assert_eq!(sanitize_filename("").err(), Some(FilenameError::Empty));
    }

    #[test]
    fn sanitize_filename_accepts_underscore_and_dash() {
        assert_eq!(sanitize_filename("my-model_v2.gguf").unwrap(), "my-model_v2.gguf");
    }

    #[test]
    fn sanitize_filename_rejects_multiple_segments() {
        // 含子目录的路径应被拒绝（仅允许单层文件名）
        assert_eq!(sanitize_filename("sub/model.gguf").err(), Some(FilenameError::Subdirectory));
        assert_eq!(sanitize_filename("sub\\model.gguf").err(), Some(FilenameError::Subdirectory));
    }
}
