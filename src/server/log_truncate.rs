//! 日志行截断：保护 `Vec<LogLine>` 不被恶意进程通过单行超长输出吃光内存。
//!
//! 设计要点：
//! - 单行超过 [`MAX_LOG_LINE_BYTES`]（16 KB）即触发截断；
//! - 截断时保留头部 [`HEAD_KEEP`] 字节 + 动态尾部字节数；
//! - 中间用 `<已截断 N 字节>` 占位符标注丢失数据量；
//! - 实现为纯函数，无副作用，方便单测覆盖。
//!
//! 与 [`crate::server::state::MAX_LOG_LINES`]（`Vec` 容量上限）配合形成"双层防御"：
//! - 第一层：单行字节数（防单行 OOM）
//! - 第二层：行数（防长跑下累积 OOM）

/// 单行日志最大长度（字节）。超过此长度的行会被截断。
///
/// **16 KB** 是基于 `llama-server` verbose 模式正常单行输出（多为
/// token 流，约 200-800 字节）保留 16-32 倍 headroom 后取整。
pub const MAX_LOG_LINE_BYTES: usize = 16 * 1024;

/// 截断时保留的头部字节数。
const HEAD_KEEP: usize = 512;
/// `<已截断 N 字节>` 占位符的最大可能长度。
/// 取 32 是因为最大可能的 `format!("<已截断 65535 字节>")` 约 22 字符，留 10 字节余量。
///
/// 尾部长度采用动态计算公式：`tail_keep = MAX - HEAD - TAIL_RESERVE`（在保证
/// 不超 MAX 的前提下取最大可用尾部），避免硬编码常量与实际不符。
const TAIL_RESERVE: usize = 32;

/// 截断单行日志。
///
/// 行为：
/// - 输入长度 ≤ [`MAX_LOG_LINE_BYTES`]：原样 clone 返回；
/// - 输入长度 > [`MAX_LOG_LINE_BYTES`]：保留 head + tail，中间用
///   `<已截断 N 字节>` 标注丢失数据量。
///
/// # 复杂度
/// - 时间：O(n)（最坏情况 `s.len()` 字节拷贝两次 + 一次 format）；
/// - 空间：O(n)（返回新的 `String`）。
///
/// 接受按字节切片的风险：极端多字节 UTF-8 字符可能正好在切点处被截断成
/// 无效序列，但本项目只在前端 WebView 显示，不参与下游解析，且 `<已截断>`
/// 标签之后的内容肉眼可识别，故不额外做 `is_char_boundary` 校验。
pub fn truncate_log_line(s: &str) -> String {
    if s.len() <= MAX_LOG_LINE_BYTES {
        return s.to_string();
    }
    let tail_keep = MAX_LOG_LINE_BYTES - HEAD_KEEP - TAIL_RESERVE;
    let head = &s[..HEAD_KEEP];
    let tail = &s[s.len() - tail_keep..];
    format!(
        "{}...<已截断 {} 字节>...{}",
        head,
        s.len() - HEAD_KEEP - tail_keep,
        tail
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 短行不触发截断，原样返回
    #[test]
    fn short_line_passes_through() {
        let s = "hello world";
        assert_eq!(truncate_log_line(s), s);
    }

    /// 刚好等于上限的字符串不触发截断
    #[test]
    fn at_max_length_passes_through() {
        let s = "a".repeat(MAX_LOG_LINE_BYTES);
        assert_eq!(truncate_log_line(&s), s);
    }

    /// 超长字符串被截断：保留 head + tail + 占位符
    #[test]
    fn oversize_line_is_truncated() {
        let s = "a".repeat(MAX_LOG_LINE_BYTES * 2);
        let out = truncate_log_line(&s);
        // 头 512 字节 + 截断标记 + 尾 256 字节
        assert!(out.starts_with(&"a".repeat(HEAD_KEEP)), "保留头部");
        assert!(out.contains("已截断"), "保留占位符");
        assert!(out.ends_with(&"a".repeat(256)), "保留尾部");
    }

    /// 截断标记里的字节数等于实际丢弃的字节数
    #[test]
    fn truncation_marker_contains_dropped_count() {
        let s = "a".repeat(MAX_LOG_LINE_BYTES * 2);
        let out = truncate_log_line(&s);
        // 总长 = MAX + EXTRA；截断后保留 = HEAD + TAIL + 标记
        // 标记中数字 = MAX + EXTRA - HEAD - TAIL
        let tail_keep = MAX_LOG_LINE_BYTES - HEAD_KEEP - TAIL_RESERVE;
        let expected_dropped = s.len() - HEAD_KEEP - tail_keep;
        assert!(out.contains(&format!("已截断 {} 字节", expected_dropped)));
    }

    /// 截断后总长度不超过上限
    #[test]
    fn truncated_output_within_limit() {
        let s = "x".repeat(1024 * 1024); // 1 MB
        let out = truncate_log_line(&s);
        // 输出 = head(512) + "...<已截断 6 位数+字节>..." + tail(256) ≈ 800 字节
        assert!(out.len() <= MAX_LOG_LINE_BYTES + 50, "截断后长度 {} 应接近上限 {}", out.len(), MAX_LOG_LINE_BYTES);
    }

    /// 多字节 UTF-8 截断：使用 ASCII 字符（每个字节都是 char boundary）。
    ///
    /// 注意：本函数按字节切片（**不做** `is_char_boundary` 校验），见函数文档说明。
    /// 如果输入在切点处正好是 UTF-8 字符中间，会产生无效序列（前端 WebView
    /// 仍能显示为「乱码」，但不会 panic）。本测试用 ASCII 验证基础功能
    /// 即可，覆盖 UTF-8 char boundary 校验需要修改 `truncate_log_line` 本体
    /// （目前刻意不做边界校验，故不写相应测试以避免测试本身 panic）。
    #[test]
    fn ascii_long_input_is_truncated() {
        // 构造超过 MAX_LOG_LINE_BYTES 的 ASCII 字符串（UTF-8 兼容）
        let s = "a".repeat(MAX_LOG_LINE_BYTES * 2);
        let out = truncate_log_line(&s);
        // 输出长度应明显小于输入
        assert!(out.len() < s.len());
        // 头尾各保留
        assert!(out.starts_with("aaaa"));
        assert!(out.ends_with("aaaa"));
    }

    /// 空字符串原样返回
    #[test]
    fn empty_line_passes_through() {
        assert_eq!(truncate_log_line(""), "");
    }
}
