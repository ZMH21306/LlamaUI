//! llama.cpp 版本解析与比较。
//!
//! llama.cpp 版本号格式：`bNNNNN`（构建号）或语义版本 `X.Y.Z`。
//! 本模块负责：
//! - 解析 llama-server 版本输出
//! - 构造 GitHub Release tag
//! - 版本比较

/// 解析 llama-server 输出的版本号
///
/// 常见输出格式：
/// - `llama-server version: b10809 (commit abc123) built with ...`
/// - `llama-server version: 0.4.0-dev (build 10809, commit 5266f24da) built with Clang`
/// - `llama-server version: 0.4.0`
pub fn parse_llama_version(output: &str) -> Option<String> {
    let text = output.trim();

    // 1) 优先匹配构建号 bNNNNN
    if let Some(idx) = text.find("version: b") {
        let after = &text[idx + "version: b".len()..];
        let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        return Some(format!("b{}", &after[..end]));
    }

    // 2) "llama-server bNNNNN"
    if let Some(idx) = text.find("llama-server b") {
        let after = &text[idx + "llama-server b".len()..];
        let end = after.find(|c: char| !c.is_ascii_alphanumeric() && c != '.').unwrap_or(after.len());
        return Some(format!("b{}", &after[..end]));
    }

    // 3) 语义版本 vX.Y.Z
    if let Some(idx) = text.find("version: ") {
        let after = &text[idx + "version: ".len()..];
        let end = after.find(|c: char| c == ' ' || c == '(' || c == '\n').unwrap_or(after.len());
        let v = after[..end].trim().trim_start_matches('v');
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }

    // 4) "llama-server vX.Y.Z"
    if let Some(idx) = text.find("llama-server v") {
        let after = &text[idx + "llama-server v".len()..];
        let end = after.find(|c: char| !c.is_ascii_alphanumeric() && c != '.').unwrap_or(after.len());
        return Some(after[..end].to_string());
    }

    None
}

/// 构造 GitHub Release tag
///
/// - `bNNNNN` → `bNNNNN`
/// - 语义版本 → `vX.Y.Z`
pub fn to_release_tag(version: &str) -> String {
    if version.starts_with('b') || version.starts_with("b") {
        version.to_string()
    } else if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{}", version)
    }
}

/// 版本比较结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionCmp {
    Less,
    Equal,
    Greater,
}

/// 比较两个 llama.cpp 版本字符串
///
/// - `bNNNNN` 构建号按数字比较
/// - 语义版本按段比较
/// - 混合格式：构建号 > 语义版本
pub fn compare_versions(a: &str, b: &str) -> VersionCmp {
    // 构建号比较
    if a.starts_with('b') && b.starts_with('b') {
        let a_num = a[1..].parse::<u64>().unwrap_or(0);
        let b_num = b[1..].parse::<u64>().unwrap_or(0);
        return if a_num < b_num {
            VersionCmp::Less
        } else if a_num > b_num {
            VersionCmp::Greater
        } else {
            VersionCmp::Equal
        };
    }

    // 语义版本比较
    let a_parts: Vec<u64> = a
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    let b_parts: Vec<u64> = b
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    for (a_val, b_val) in a_parts.iter().zip(b_parts.iter()) {
        if a_val < b_val {
            return VersionCmp::Less;
        } else if a_val > b_val {
            return VersionCmp::Greater;
        }
    }

    match a_parts.len().cmp(&b_parts.len()) {
        std::cmp::Ordering::Less => VersionCmp::Less,
        std::cmp::Ordering::Greater => VersionCmp::Greater,
        std::cmp::Ordering::Equal => VersionCmp::Equal,
    }
}

/// 检查版本 a 是否比 b 新
pub fn is_newer(a: &str, b: &str) -> bool {
    matches!(compare_versions(a, b), VersionCmp::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_build_version() {
        let text = "llama-server version: b10809 (commit abc123) built with Clang";
        assert_eq!(parse_llama_version(text), Some("b10809".to_string()));
    }

    #[test]
    fn parse_semver_version() {
        let text = "llama-server version: 0.4.0";
        assert_eq!(parse_llama_version(text), Some("0.4.0".to_string()));
    }

    #[test]
    fn parse_semver_with_build() {
        let text = "llama-server version: 0.4.0-dev (build 10809, commit 5266f24da) built with Clang";
        assert_eq!(parse_llama_version(text), Some("0.4.0-dev".to_string()));
    }

    #[test]
    fn to_tag_build() {
        assert_eq!(to_release_tag("b10809"), "b10809");
    }

    #[test]
    fn to_tag_semver() {
        assert_eq!(to_release_tag("0.4.0"), "v0.4.0");
    }

    #[test]
    fn compare_build_numbers() {
        assert_eq!(compare_versions("b10809", "b10808"), VersionCmp::Greater);
        assert_eq!(compare_versions("b10809", "b10809"), VersionCmp::Equal);
        assert_eq!(compare_versions("b10809", "b10900"), VersionCmp::Less);
    }

    #[test]
    fn compare_semver() {
        assert_eq!(compare_versions("0.4.0", "0.3.0"), VersionCmp::Greater);
        assert_eq!(compare_versions("0.4.0", "0.4.0"), VersionCmp::Equal);
        assert_eq!(compare_versions("0.3.0", "0.4.0"), VersionCmp::Less);
    }
}