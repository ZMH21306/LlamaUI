//! 时间戳工具。
//!
//! 集中所有"时间戳格式化"逻辑，避免散落在各模块的字符串硬编码。

use chrono::Local;

/// 格式 `YYYY-MM-DD HH:MM:SS.mmm`，按本地时区。
///
/// 这是 `LogLine::timestamp` 的唯一来源，**前端 JS 也按此格式解析**（见
/// `dist/main.js` 的 `parseTimestamp`）。改动需要同步前端。
pub fn now_ts() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ts_format_is_stable() {
        // 不验证具体值（依赖系统时间），仅验证格式
        let s = now_ts();
        assert_eq!(
            s.len(),
            23,
            "格式必须为 YYYY-MM-DD HH:MM:SS.mmm：got `{}`",
            s
        );
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], " ");
        assert_eq!(&s[13..14], ":");
        assert_eq!(&s[16..17], ":");
        assert_eq!(&s[19..20], ".");
    }
}
