//! 镜像/代理URL支持模块。
//!
//! 当 GitHub Releases 访问不可靠时，自动切换到备用镜像。
//! 支持的镜像列表：
//! - jsDelivr: https://cdn.jsdelivr.net/gh/ggml-org/llama.cpp@...
//! - GitHub Proxy: https://hub.gitmirror.com/ggml-org/llama.cpp/releases/download/...
//!
//! 环境变量：
//! - `LLAMAUI_MIRROR`：启用镜像模式 (true/false)
//! - `LLAMAUI_CUSTOM_MIRROR`：自定义镜像前缀

#![allow(clippy::module_name_repetitions)]

use std::env;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

/// GitHub Releases URL 正则（编译一次）
static RELEASE_RE: OnceLock<regex::Regex> = OnceLock::new();

fn get_release_regex() -> &'static regex::Regex {
    RELEASE_RE.get_or_init(|| {
        regex::Regex::new(
            r"^https://github\.com/([^/]+/[^/]+)/releases/download/([^/]+)/(.+)$"
        ).unwrap()
    })
}

/// 成功计数器
static SUCCESS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 失败计数器
static FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 镜像配置
#[derive(Debug, Clone)]
pub struct MirrorConfig {
    /// 是否启用镜像模式
    pub enabled: bool,
    /// 自定义镜像前缀（如果设置则优先使用）
    pub custom_prefix: Option<String>,
    /// 镜像URL前缀列表（按优先级排序）
    pub mirrors: Vec<String>,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        let enabled = env::var("LLAMAUI_MIRROR")
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(false);

        let custom_prefix = env::var("LLAMAUI_CUSTOM_MIRROR").ok();

        let mirrors = vec![
            // GitHub 官方 CDN
            "https://github.com".to_string(),
            // jsDelivr
            "https://cdn.jsdelivr.net/gh".to_string(),
            // GitHub 代理镜像（华人地区）
            "https://hub.gitmirror.com".to_string(),
        ];

        Self {
            enabled,
            custom_prefix,
            mirrors,
        }
    }
}

impl MirrorConfig {
    /// 获取当前镜像配置
    pub fn get() -> Self {
        Self::default()
    }

    /// 根据原始 GitHub URL 生成镜像 URL
    ///
    /// 支持的转换：
    /// - GitHub → jsDelivr: `https://github.com/owner/repo/releases/download/tag/asset`
    ///   → `https://cdn.jsdelivr.net/gh/owner/repo@tag/asset`
    pub fn build_url_from_github(&self, github_url: &str) -> String {
        // 如果设置了自定义镜像前缀，直接使用
        if let Some(custom) = &self.custom_prefix {
            return github_url.replace("https://github.com", custom);
        }

        // 如果没有启用镜像，直接返回原始 URL
        if !self.enabled {
            return github_url.to_string();
        }

        // GitHub Releases URL 格式：
        // https://github.com/{owner}/{repo}/releases/download/{tag}/{asset}
        if let Some(cap) = get_release_regex().captures(github_url) {
            let repo = &cap[1];
            let tag = &cap[2];
            let asset = &cap[3];

            // 使用 jsdelivr 作为主镜像（全局可用性好）
            // https://github.com/owner/repo/releases/download/tag/asset
            //   → https://cdn.jsdelivr.net/gh/owner/repo@tag/asset
            return format!(
                "https://cdn.jsdelivr.net/gh/{}@{}/{}",
                repo, tag, asset
            );
        }

        // 如果不匹配，直接返回原 URL
        github_url.to_string()
    }

    /// 获取下一个候选镜像 URL（用于重试时切换镜像）
    pub fn get_next_mirror(&self, current_url: &str, github_url: &str) -> Option<String> {
        if self.mirrors.is_empty() {
            return None;
        }

        // 找到当前使用的镜像索引
        let current_idx = self.mirrors.iter().position(|m| current_url.contains(m))?;

        // 尝试下一个镜像
        self.mirrors.iter().enumerate().find_map(|(idx, mirror)| {
            if idx != current_idx {
                Some(github_url.replace("https://github.com", mirror))
            } else {
                None
            }
        })
    }

    /// 记录成功反馈
    pub fn record_success() {
        SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录失败反馈
    pub fn record_failure() {
        FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    /// 获取镜像健康情况
    pub fn get_stats() -> (usize, usize) {
        (
            SUCCESS_COUNT.load(Ordering::Relaxed),
            FAIL_COUNT.load(Ordering::Relaxed),
        )
    }

    /// 重置统计
    pub fn reset_stats() {
        SUCCESS_COUNT.store(0, Ordering::Relaxed);
        FAIL_COUNT.store(0, Ordering::Relaxed);
    }
}

/// 根据版本选择性地应用镜像
/// 对于 nightly 或 prerelease 版本优先使用 jsDelivr 镜像
pub fn should_use_mirror(tag: &str) -> bool {
    let config = MirrorConfig::get();
    if !config.enabled {
        return false;
    }

    // nightly 版本使用镜像
    tag.starts_with("b") || tag.starts_with("nightly")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mirror_config_disabled() {
        let config = MirrorConfig {
            enabled: false,
            custom_prefix: None,
            mirrors: vec![],
        };
        let url = "https://github.com/owner/releases/download/v0.4.2/llama-linux-x64.zip";
        assert_eq!(config.build_url_from_github(url), url);
    }

    #[test]
    fn test_mirror_config_with_jsdelivr() {
        let config = MirrorConfig {
            enabled: true,
            custom_prefix: None,
            mirrors: vec!["https://github.com".to_string()],
        };
        let github_url = "https://github.com/ggml-org/llama.cpp/releases/download/v0.4.2/llama-linux-x64.zip";
        let mirrored = config.build_url_from_github(github_url);
        // enabled=true 时应转换为 jsDelivr URL
        assert!(mirrored.starts_with("https://cdn.jsdelivr.net/gh/ggml-org/llama.cpp@v0.4.2/"), "{}", mirrored)
    }

    #[test]
    fn test_mirror_stats() {
        MirrorConfig::reset_stats();
        assert_eq!(MirrorConfig::get_stats(), (0, 0));
    }
}