//! 全局常量集中管理模块。
//!
//! 将各模块中提取的魔法数字统一收纳，便于：
//! - 单一事实来源（Single Source of Truth）
//! - 跨模块共享（如 HTTP 超时）
//! - 性能调优时统一调整

// ---- HTTP 超时（供 util/http.rs 和 download/llama_downloader.rs 共享） ----

/// HTTP 请求默认读取超时（秒）。
pub const HTTP_TIMEOUT_SECS: u64 = 60;
/// HTTP 连接超时（秒）。
pub const HTTP_CONNECT_TIMEOUT_SECS: u64 = 15;

// ---- 下载器超时 ----

/// 共享 HTTP 客户端整体超时（秒）。
pub const SHARED_CLIENT_TIMEOUT_SECS: u64 = 300;
/// 共享 HTTP 客户端连接超时（秒）。
pub const SHARED_CLIENT_CONNECT_TIMEOUT_SECS: u64 = 30;
/// 共享 HTTP 客户端连接池空闲超时（秒）。
pub const SHARED_CLIENT_POOL_IDLE_TIMEOUT_SECS: u64 = 90;
/// HEAD 请求超时（秒）。
pub const HEAD_TIMEOUT_SECS: u64 = 15;
/// 带宽探测超时（秒）。
pub const BANDWIDTH_PROBE_TIMEOUT_SECS: u64 = 10;
/// 分块超时下限（秒）。
pub const MIN_CHUNK_TIMEOUT_SECS: u64 = 60;
/// 分块超时上限（秒）。
pub const MAX_CHUNK_TIMEOUT_SECS: u64 = 600;
/// 初始重试退避基数（毫秒）。
pub const RETRY_BASE_DELAY_MS: u64 = 500;
/// 初始重试退避上限（秒）。
pub const RETRY_MAX_DELAY_SECS: u64 = 2;
/// 重试轮次退避基数（毫秒）。
pub const RETRY_BACKOFF_BASE_MS: u64 = 200;
/// GitHub Release API 超时（秒）。
pub const GITHUB_RELEASE_TIMEOUT_SECS: u64 = 45;
/// GitHub Release API 连接超时（秒）。
pub const GITHUB_RELEASE_CONNECT_TIMEOUT_SECS: u64 = 8;
/// 资产探测超时（秒）。
pub const ASSET_PROBE_TIMEOUT_SECS: u64 = 6;
/// 资产探测连接超时（秒）。
pub const ASSET_PROBE_CONNECT_TIMEOUT_SECS: u64 = 4;

// ---- 路径安全 ----

/// URL 长度上限（字节）。防止异常大的 payload 攻击。
pub const MAX_URL_BYTES: usize = 2048;
/// 高风险目录段名。出现在父目录末段或中间段都视为可疑。
pub const RISKY_DIR_SEGMENTS: &[&str] = &["tmp", "temp", "downloads"];
/// 安全相关路径组件的名称。禁止使用这些名称作为目录或文件名。
pub const SAFE_PATH_COMPONENTS: &[&str] = &[
    "..", ".", "null", "con", "prn", "aux", "nul", "com1", "lpt1",
];
/// 可执行文件名白名单（大小写不敏感，自动追加 `.exe` 变体）。
pub const ALLOWED_LLAMA_EXECUTABLE_NAMES: &[&str] = &["llama-server"];
/// 路径标准化后用于比较的最大长度。
pub const MAX_PATH_LENGTH_FOR_COMPARE: usize = 4096;

// ---- 日志截断 ----

/// 单行日志最大长度（字节）。超过此长度的行会被截断。
pub const MAX_LOG_LINE_BYTES: usize = 16 * 1024;
/// 截断时保留的头部字节数。
pub const HEAD_KEEP: usize = 512;
/// `<已截断 N 字节>` 占位符的最大可能长度。
pub const TAIL_RESERVE: usize = 32;

// ---- GPU 指标缓存 ----

/// GPU 指标缓存时间（秒）。
pub const GPU_CACHE_TTL_SECS: u64 = 5;
/// GPU 指标缓存时间（Duration）。
pub const GPU_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(GPU_CACHE_TTL_SECS);
/// 底层指标采样间隔（毫秒）。
pub const METRICS_INTERVAL_MS: u64 = 100;

// ---- 服务状态 ----

/// 内存中最大日志行数。
pub const MAX_LOG_LINES: usize = 5000;