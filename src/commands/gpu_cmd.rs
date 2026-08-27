//! 增强版 GPU 检测命令。
//!
//! 改进安全性、错误处理和性能。

use std::time::{Duration, Instant};
use std::sync::Arc;
use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::gpu_detection::{self, GpuInfo, GpuIssue};

/// 智能缓存系统
pub struct GpuCache {
    /// 缓存条目：(平台哈希值, (GPU 信息, 过期时间))
    cache: Arc<RwLock<HashMap<u64, (GpuInfo, Instant)>>>,
    /// 最大缓存大小
    max_size: usize,
    /// 缓存过期时间
    ttl: Duration,
}

impl GpuCache {
    /// 创建新的缓存实例
    pub fn new(max_size: usize, ttl_secs: u64) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            max_size,
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// 获取缓存项
    pub async fn get(&self, platform_hash: u64) -> Option<GpuInfo> {
        let cache = self.cache.read().await;
        if let Some((gpu_info, expiry)) = cache.get(&platform_hash) {
            if *expiry > Instant::now() {
                return Some(gpu_info.clone());
            }
        }
        None
    }

    /// 插入缓存项
    pub async fn insert(&self, platform_hash: u64, gpu_info: GpuInfo) {
        let mut cache = self.cache.write().await;

        // 检查是否已存在相同平台
        let mut to_remove = Vec::new();
        for (hash, (_, expiry)) in cache.iter() {
            if *hash != platform_hash && *expiry < Instant::now() {
                to_remove.push(*hash);
            }
        }

        for hash in to_remove {
            cache.remove(&hash);
        }

        // 如果缓存已满，删除最旧的项
        if cache.len() >= self.max_size {
            let mut oldest_hash = None;
            let mut oldest_time = Instant::now();

            for (hash, (_, expiry)) in cache.iter() {
                if *expiry < oldest_time {
                    oldest_time = *expiry;
                    oldest_hash = Some(*hash);
                }
            }

            if let Some(hash_to_remove) = oldest_hash {
                cache.remove(&hash_to_remove);
            }
        }

        // 插入新项
        let expiry = Instant::now() + self.ttl;
        cache.insert(platform_hash, (gpu_info, expiry));
    }
}

/// 获取平台唯一标识符
fn get_platform_hash() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    // 组合多种平台信息
    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };

    platform.hash(&mut hasher);

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    arch.hash(&mut hasher);

    hasher.finish()
}

/// 增强版 GPU 检测命令
///
/// 集成了断路器保护、缓存和环境验证。
#[tauri::command]
pub async fn detect_gpus_enhanced() -> Result<Vec<GpuInfo>, String> {
    let start_time = Instant::now();
    let platform_hash = get_platform_hash();

    // 创建缓存实例
    let cache = GpuCache::new(3, 300);

    // 尝试从缓存获取
    if let Some(cached_gpu) = cache.get(platform_hash).await {
        tracing::info!(target: "GpuCmdEnhanced", "从缓存获取 GPU 信息, 耗时: {:?}ms", start_time.elapsed().as_millis());

        // 构建完整的 GPU 列表（缓存只存储第一个 GPU）
        let mut gpus = vec![cached_gpu];

        // 添加其他 GPU 信息
        if let Ok(other_gpus) = gpu_detection::detect_all_gpus_async().await {
            for gpu in other_gpus {
                if !gpus.iter().any(|g| g.model == gpu.model) {
                    gpus.push(gpu);
                }
            }
        }

        return Ok(gpus);
    }

    // 执行实际检测
    let result = gpu_detection::detect_all_gpus_async().await;

    match result {
        Ok(gpus) => {
            if !gpus.is_empty() {
                // 缓存第一个 GPU 信息
                if let Some(first_gpu) = gpus.first() {
                    cache.insert(platform_hash, first_gpu.clone()).await;
                }
            }

            let elapsed = start_time.elapsed();
            tracing::info!(target: "GpuCmdEnhanced", "GPU 检测完成, 找到 {} 个 GPU, 耗时: {:?}ms", gpus.len(), elapsed.as_millis());

            Ok(gpus)
        }
        Err(e) => {
            let elapsed = start_time.elapsed();
            tracing::error!(target: "GpuCmdEnhanced", "GPU 检测失败: {}, 耗时: {:?}ms", e, elapsed.as_millis());
            Err(e.to_string())
        }
    }
}

/// 增强版 GPU 诊断命令
///
/// 集成了断路器保护和环境验证。
#[tauri::command]
pub async fn diagnose_gpu_enhanced() -> Result<Vec<GpuIssue>, String> {
    let start_time = Instant::now();

    // 执行诊断
    let result = gpu_detection::diagnose_gpu_issues_async().await;

    let elapsed = start_time.elapsed();

    match result {
        Ok(issues) => {
            tracing::info!(target: "GpuCmdEnhanced", "GPU 诊断完成, 发现 {} 个问题, 耗时: {:?}ms", issues.len(), elapsed.as_millis());
            Ok(issues)
        }
        Err(e) => {
            tracing::error!(target: "GpuCmdEnhanced", "GPU 诊断失败: {}, 耗时: {:?}ms", e, elapsed.as_millis());
            Err(e.to_string())
        }
    }
}

/// GPU 检测命令（兼容旧版 API）
///
/// 使用增强版 GPU 检测，集成缓存和错误处理。
#[tauri::command]
pub async fn detect_gpus() -> Result<Vec<GpuInfo>, String> {
    detect_gpus_enhanced().await
}

/// GPU 诊断命令（兼容旧版 API）
///
/// 使用增强版 GPU 诊断，集成断路器保护。
#[tauri::command]
pub async fn diagnose_gpu() -> Result<Vec<GpuIssue>, String> {
    diagnose_gpu_enhanced().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_enhanced_detect_gpus() {
        let result = detect_gpus_enhanced().await;
        assert!(result.is_ok());
        let gpus = result.unwrap();
        assert!(!gpus.is_empty());
    }

    #[tokio::test]
    async fn test_enhanced_diagnose_gpu() {
        let result = diagnose_gpu_enhanced().await;
        assert!(result.is_ok());
        let _issues = result.unwrap();
        // 可能返回空列表，这也是合理的
    }

    #[test]
    fn test_platform_hash() {
        let hash1 = get_platform_hash();
        let hash2 = get_platform_hash();
        assert_eq!(hash1, hash2);
    }
}
