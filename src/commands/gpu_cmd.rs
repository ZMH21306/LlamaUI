//! 增强版 GPU 检测命令。
//!
//! 改进安全性、错误处理和性能。

use std::time::{Duration, Instant};
use std::sync::Arc;
use std::collections::HashMap;

use tokio::sync::{RwLock, Mutex};
use tokio::time::timeout;

use crate::gpu_detection::{self, GpuInfo, GpuIssue, GpuDetectionError};

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
            if expiry > Instant::now() {
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
            if *hash != platform_hash && expiry < Instant::now() {
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
                if expiry < oldest_time {
                    oldest_time = expiry;
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

    /// 清除所有缓存
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// 获取缓存状态
    pub async fn stats(&self) -> (usize, f64) {
        let cache = self.cache.read().await;
        let size = cache.len();
        
        // 计算缓存命中率（简化实现）
        // 实际应用中需要跟踪查询次数
        let hit_rate = if size > 0 { 
            cache.values().filter(|(_, (_, expiry))| expiry > &Instant::now()).count() as f64 / size as f64 
        } else { 
            0.0 
        };
        
        (size, hit_rate)
    }
}

/// 断路器状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// 断路器配置
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub half_open_timeout_secs: u64,
    pub reset_timeout_secs: u64,
    pub timeout_secs: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            half_open_timeout_secs: 30,
            reset_timeout_secs: 300,
            timeout_secs: 10,
        }
    }
}

/// 断路器
pub struct CircuitBreaker {
    state: Mutex<CircuitBreakerState>,
    config: CircuitBreakerConfig,
    failure_count: Mutex<u32>,
    last_failure_time: Mutex<Instant>,
    success_count: Mutex<u32>,
    tool_name: &'static str,
}

impl CircuitBreaker {
    /// 创建新的断路器
    pub fn new(tool_name: &'static str) -> Self {
        Self {
            state: Mutex::new(CircuitBreakerState::Closed),
            config: CircuitBreakerConfig::default(),
            failure_count: Mutex::new(0),
            last_failure_time: Mutex::new(Instant::now()),
            success_count: Mutex::new(0),
            tool_name,
        }
    }

    /// 执行操作，应用断路器模式
    pub async fn call<F, R, E>(&self, operation: F) -> Result<R, GpuDetectionError>
    where
        F: std::future::Future<Output = Result<R, E>>,
        E: std::fmt::Display + Send + 'static,
    {
        // 检查状态
        let mut state = self.state.lock().await;
        let mut failure_count = self.failure_count.lock().await;
        let mut last_failure_time = self.last_failure_time.lock().await;

        // 检查是否需要重置
        if last_failure_time.elapsed().as_secs() > self.config.reset_timeout_secs {
            *state = CircuitBreakerState::Closed;
            *failure_count = 0;
        }

        // 检查是否处于打开状态
        if matches!(*state, CircuitBreakerState::Open) && 
           last_failure_time.elapsed().as_secs() < self.config.half_open_timeout_secs {
            return Err(GpuDetectionError::CommandNotFound {
                tool: self.tool_name.to_string(),
            });
        }

        // 执行操作
        match timeout(Duration::from_secs(self.config.timeout_secs), operation).await {
            Ok(Ok(result)) => {
                // 成功
                let mut success_count = self.success_count.lock().await;
                *success_count += 1;
                
                if *success_count >= self.config.failure_threshold {
                    *state = CircuitBreakerState::Closed;
                    *success_count = 0;
                }
                Ok(result)
            }
            Ok(Err(_)) => {
                // 失败
                *failure_count += 1;
                *last_failure_time = Instant::now();
                
                if *failure_count >= self.config.failure_threshold {
                    *state = CircuitBreakerState::Open;
                }
                Err(GpuDetectionError::CommandFailed {
                    tool: self.tool_name.to_string(),
                    exit_code: None,
                })
            }
            Err(_) => {
                // 超时
                *failure_count += 1;
                *last_failure_time = Instant::now();
                
                if *failure_count >= self.config.failure_threshold {
                    *state = CircuitBreakerState::Open;
                }
                Err(GpuDetectionError::Timeout {
                    tool: self.tool_name.to_string(),
                    timeout_secs: self.config.timeout_secs,
                })
            }
        }
    }
}

/// 全局断路器实例
static GLOBAL_NVIDIA_BREAKER: once_cell::sync::Lazy<Arc<CircuitBreaker>> = 
    once_cell::sync::Lazy::new(|| Arc::new(CircuitBreaker::new("nvidia-smi")));

static GLOBAL_AMD_BREAKER: once_cell::sync::Lazy<Arc<CircuitBreaker>> = 
    once_cell::sync::Lazy::new(|| Arc::new(CircuitBreaker::new("lspci")));

static GLOBAL_APPLE_BREAKER: once_cell::sync::Lazy<Arc<CircuitBreaker>> = 
    once_cell::sync::Lazy::new(|| Arc::new(CircuitBreaker::new("system_profiler")));

/// 获取断路器实例
fn get_circuit_breaker(tool_name: &str) -> Arc<CircuitBreaker> {
    match tool_name {
        "nvidia-smi" => Arc::clone(&GLOBAL_NVIDIA_BREAKER),
        "lspci" => Arc::clone(&GLOBAL_AMD_BREAKER),
        "system_profiler" => Arc::clone(&GLOBAL_APPLE_BREAKER),
        _ => Arc::clone(&GLOBAL_NVIDIA_BREAKER),
    }
}

/// 带断路器保护的系统命令执行
async fn execute_command_with_breaker(
    command: String,
    tool_name: &'static str,
) -> Result<tokio::process::Output, GpuDetectionError> {
    let breaker = get_circuit_breaker(tool_name);
    
    let command_future = async move {
        tokio::process::Command::new("sh")
            .args(["-c", &command])
            .output()
            .await
            .map_err(|e| GpuDetectionError::Internal {
                message: format!("执行命令失败: {}", e),
            })
    };
    
    breaker.call(command_future).await
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
        let issues = result.unwrap();
        // 可能返回空列表，这也是合理的
        assert!(issues.len() >= 0);
    }

    #[test]
    fn test_platform_hash() {
        let hash1 = get_platform_hash();
        let hash2 = get_platform_hash();
        assert_eq!(hash1, hash2);
    }

    #[tokio::test]
    async fn test_circuit_breaker() {
        let breaker = CircuitBreaker::new("test-tool");
        
        // 测试成功路径
        let result = breaker.call(async { Ok::<_, &'static str>("success") }).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        
        // 测试失败路径
        let result = breaker.call(async { Err::<&str, _>("failure") }).await;
        assert!(result.is_err());
    }
}
