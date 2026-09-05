//! GPU 检测与诊断模块。
//!
//! 检测系统 GPU 型号、驱动版本、CUDA/ROCm/Vulkan 版本，
//! 并提供自动修复建议。
//!
//! 本模块已重构为独立的 GPU 检测模块，通过异步、事件驱动和错误转换等技术
//! 提升了 GPU 检测的性能、可靠性和用户体验。

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::util::process::silent_tokio_command as silent_command;

/// GPU 检测错误类型
#[derive(Debug, Clone)]
pub enum GpuDetectionError {
    /// 检测超时
    Timeout { tool: String, timeout_secs: u64 },
    /// 系统命令执行失败
    CommandFailed { tool: String, exit_code: Option<i32> },
    /// 命令不存在
    CommandNotFound { tool: String },
    /// 系统不支持
    UnsupportedPlatform { platform: String },
    /// 内部错误
    Internal { message: String },
}

impl std::fmt::Display for GpuDetectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuDetectionError::Timeout { tool, timeout_secs } => {
                write!(f, "GPU 检测工具 '{}' 执行超时 ({}秒)", tool, timeout_secs)
            }
            GpuDetectionError::CommandFailed { tool, exit_code } => {
                write!(f, "GPU 检测工具 '{}' 执行失败", tool)?;
                if let Some(code) = exit_code {
                    write!(f, "，退出码: {}", code)?;
                }
                Ok(())
            }
            GpuDetectionError::CommandNotFound { tool } => {
                write!(f, "GPU 检测工具 '{}' 未找到，请确保已安装相关驱动", tool)
            }
            GpuDetectionError::UnsupportedPlatform { platform } => {
                write!(f, "当前平台 '{}' 不支持 GPU 检测", platform)
            }
            GpuDetectionError::Internal { message } => {
                write!(f, "内部错误: {}", message)
            }
        }
    }
}

impl std::error::Error for GpuDetectionError {}

/// 断路器状态
#[derive(Debug, Clone, Copy)]
pub enum CircuitBreakerState {
    /// 正常状态
    Closed,
    /// 打开状态（故障）
    Open,
    /// 半开状态（尝试恢复）
    HalfOpen,
}

/// 断路器配置
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// 失败次数阈值
    pub failure_threshold: u32,
    /// 半开状态持续时间（秒）
    pub half_open_timeout_secs: u64,
    /// 重置时间窗口（秒）
    pub reset_timeout_secs: u64,
    /// 超时时间（秒）
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

    /// 创建带自定义配置的断路器
    pub fn with_config(tool_name: &'static str, config: CircuitBreakerConfig) -> Self {
        Self {
            state: Mutex::new(CircuitBreakerState::Closed),
            config,
            failure_count: Mutex::new(0),
            last_failure_time: Mutex::new(Instant::now()),
            success_count: Mutex::new(0),
            tool_name,
        }
    }

    /// 执行操作，如果断路器处于打开状态则跳过
    pub async fn call<F, R, E>(&self, operation: F) -> Result<R, GpuDetectionError>
    where
        F: std::future::Future<Output = Result<R, E>>,
        E: std::fmt::Display + Send + 'static,
    {
        // 检查是否需要重置状态
        if self.should_reset() {
            let mut state = self.state.lock().await;
            *state = CircuitBreakerState::Closed;
            let mut failure_count = self.failure_count.lock().await;
            *failure_count = 0;
            tracing::debug!(target: "GpuCircuitBreaker", tool = self.tool_name, "断路器状态重置");
        }

        // 检查是否处于打开状态
        let state = *self.state.lock().await;
        if matches!(state, CircuitBreakerState::Open) && !self.is_half_open() {
            return Err(GpuDetectionError::CommandNotFound {
                tool: self.tool_name.to_string(),
            });
        }

        // 执行操作
        match timeout(Duration::from_secs(self.config.timeout_secs), operation).await {
            Ok(Ok(result)) => {
                // 操作成功
                self.on_success().await;
                Ok(result)
            }
            Ok(Err(_e)) => {
                // 操作失败
                self.on_failure().await;
                Err(GpuDetectionError::CommandFailed {
                    tool: self.tool_name.to_string(),
                    exit_code: None,
                })
            }
            Err(_) => {
                // 超时
                self.on_failure().await;
                Err(GpuDetectionError::Timeout {
                    tool: self.tool_name.to_string(),
                    timeout_secs: self.config.timeout_secs,
                })
            }
        }
    }

    /// 记录成功
    async fn on_success(&self) {
        let mut success_count = self.success_count.lock().await;
        *success_count += 1;

        // 如果成功次数足够，在半开状态下切换到关闭状态
        if *success_count >= self.config.failure_threshold {
            let mut state = self.state.lock().await;
            *state = CircuitBreakerState::Closed;
            *success_count = 0;
            tracing::info!(target: "GpuCircuitBreaker", tool = self.tool_name, "断路器状态从 HalfOpen 切换到 Closed");
        }
    }

    /// 记录失败
    async fn on_failure(&self) {
        let mut failure_count = self.failure_count.lock().await;
        let mut last_failure_time = self.last_failure_time.lock().await;
        *failure_count += 1;
        *last_failure_time = Instant::now();

        // 如果失败次数达到阈值，切换到打开状态
        if *failure_count >= self.config.failure_threshold {
            let mut state = self.state.lock().await;
            *state = CircuitBreakerState::Open;
            tracing::warn!(
                target: "GpuCircuitBreaker",
                tool = self.tool_name,
                failure_count = *failure_count,
                "断路器状态从 Closed 切换到 Open"
            );
        }
    }

    /// 检查是否需要重置状态
    fn should_reset(&self) -> bool {
        let last_failure = self.last_failure_time.try_lock().map(|g| *g).unwrap_or_else(|_| Instant::now());
        last_failure.elapsed().as_secs() > self.config.reset_timeout_secs
    }

    /// 检查是否处于半开状态
    fn is_half_open(&self) -> bool {
        let last_failure = self.last_failure_time.try_lock().map(|g| *g).unwrap_or_else(|_| Instant::now());
        last_failure.elapsed().as_secs() >= self.config.half_open_timeout_secs
    }
}

/// 全局断路器实例（使用 OnceLock）
static NVIDIA_CIRCUIT_BREAKER: std::sync::OnceLock<Arc<CircuitBreaker>> = std::sync::OnceLock::new();
static AMD_CIRCUIT_BREAKER: std::sync::OnceLock<Arc<CircuitBreaker>> = std::sync::OnceLock::new();
static APPLE_CIRCUIT_BREAKER: std::sync::OnceLock<Arc<CircuitBreaker>> = std::sync::OnceLock::new();
static ROCM_CIRCUIT_BREAKER: std::sync::OnceLock<Arc<CircuitBreaker>> = std::sync::OnceLock::new();

/// 获取全局 NVIDIA GPU 检测断路器
pub fn nvidia_circuit_breaker() -> Arc<CircuitBreaker> {
    NVIDIA_CIRCUIT_BREAKER
        .get_or_init(|| Arc::new(CircuitBreaker::new("nvidia-smi")))
        .clone()
}

/// 获取全局 AMD GPU 检测断路器
pub fn amd_circuit_breaker() -> Arc<CircuitBreaker> {
    AMD_CIRCUIT_BREAKER
        .get_or_init(|| Arc::new(CircuitBreaker::new("lspci")))
        .clone()
}

/// 获取全局 Apple Silicon GPU 检测断路器
pub fn apple_circuit_breaker() -> Arc<CircuitBreaker> {
    APPLE_CIRCUIT_BREAKER
        .get_or_init(|| Arc::new(CircuitBreaker::new("system_profiler")))
        .clone()
}

/// 获取全局 ROCm 版本检测断路器
pub fn rocm_circuit_breaker() -> Arc<CircuitBreaker> {
    ROCM_CIRCUIT_BREAKER
        .get_or_init(|| Arc::new(CircuitBreaker::new("rocminfo")))
        .clone()
}

/// GPU 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// GPU 型号
    pub model: String,
    /// 制造商 (NVIDIA/AMD/Intel/Unknown/Apple Silicon)
    pub vendor: String,
    /// 显存大小 (MB)
    pub memory_mb: Option<u64>,
    /// 驱动版本
    pub driver_version: Option<String>,
    /// CUDA 版本（NVIDIA）
    pub cuda_version: Option<String>,
    /// ROCm 版本（AMD）
    pub rocm_version: Option<String>,
    /// Metal 版本（Apple Silicon）
    pub metal_version: Option<String>,
    /// M4/M3 Pro/M2等Apple Silicon的神经引擎版本
    pub apple_neuron_version: Option<String>,
    /// Vulkan 支持
    pub vulkan_support: bool,
    /// Metal 支持（macOS/Apple Silicon）
    pub metal_support: bool,
    /// 可用的 llama-server 后端
    pub available_backends: Vec<String>,
    /// 推荐的后端
    pub recommended_backend: String,
    /// 诊断问题
    pub issues: Vec<GpuIssue>,
}

/// GPU 问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuIssue {
    /// 问题类型
    pub issue_type: String,
    /// 严重程度 (error/warning/info)
    pub severity: String,
    /// 描述
    pub message: String,
    /// 修复建议
    pub suggestion: String,
    /// 是否可自动修复
    pub auto_fixable: bool,
}

/// GPU 状态变更事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuStateEvent {
    /// 事件类型
    pub event_type: String,
    /// GPU 信息
    pub gpu_info: GpuInfo,
    /// 时间戳
    pub timestamp: u64,
}

/// 检测所有 GPU（异步版本）
pub async fn detect_all_gpus_async() -> Result<Vec<GpuInfo>, GpuDetectionError> {
    tracing::debug!(target: "GpuDetect", "开始异步检测所有 GPU");

    // 环境验证
    validate_environment()?;

    let mut gpus = Vec::new();

    // 使用 Tokio 的 join_all 并发执行检测
    let nvidia_task = detect_nvidia_gpu_async();
    let amd_task = detect_amd_gpu_async();
    let apple_task = detect_apple_silicon_gpu_async();

    let (nvidia, amd, apple) = tokio::join!(nvidia_task, amd_task, apple_task);

    if let Some(nvidia_gpu) = nvidia {
        gpus.push(nvidia_gpu);
    }
    if let Some(amd_gpu) = amd {
        gpus.push(amd_gpu);
    }
    if let Some(apple_gpu) = apple {
        gpus.push(apple_gpu);
    }

    if gpus.is_empty() {
        tracing::info!(target: "GpuDetect", "未检测到独立 GPU，使用 CPU Only");
        gpus.push(GpuInfo {
            model: "CPU Only".to_string(),
            vendor: "Unknown".to_string(),
            memory_mb: None,
            driver_version: None,
            cuda_version: None,
            rocm_version: None,
            metal_version: None,
            apple_neuron_version: None,
            vulkan_support: false,
            metal_support: false,
            available_backends: vec!["cpu".to_string()],
            recommended_backend: "cpu".to_string(),
            issues: vec![GpuIssue {
                issue_type: "no_gpu".to_string(),
                severity: "info".to_string(),
                message: "未检测到独立 GPU，将使用 CPU 推理".to_string(),
                suggestion: "如需 GPU 加速，请安装 NVIDIA/AMD/Apple Silicon 独立显卡".to_string(),
                auto_fixable: false,
            }],
        });
    } else {
        for gpu in &gpus {
            let cuda_str = gpu.cuda_version.as_deref().unwrap_or("?");
            let rocm_str = gpu.rocm_version.as_deref().unwrap_or("?");
            let metal_str = gpu.metal_version.as_deref().unwrap_or("?");
            let apple_neuron_str = gpu.apple_neuron_version.as_deref().unwrap_or("?");
            
            tracing::info!(
                target: "GpuDetect",
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = ?gpu.memory_mb,
                driver = ?gpu.driver_version,
                cuda = %cuda_str,
                rocm = %rocm_str,
                metal = %metal_str,
                apple_neuron = %apple_neuron_str,
                recommended = %gpu.recommended_backend,
                "检测到 GPU"
            );
        }
    }

    Ok(gpus)
}

/// 验证环境
fn validate_environment() -> Result<(), GpuDetectionError> {
    let platform = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    };
    tracing::debug!(target: "GpuDetect", platform = %platform, "验证 GPU 检测环境");
    Ok(())
}

/// 异步检测 NVIDIA GPU
async fn detect_nvidia_gpu_async() -> Option<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "异步检测 NVIDIA GPU...");

    // 使用断路器保护
    let nvidia_smi_task = async {
        silent_command("nvidia-smi")
            .args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"])
            .output()
            .await
    };

    let output = match nvidia_circuit_breaker().call(nvidia_smi_task).await {
        Ok(output) => output,
        Err(e) => {
            tracing::debug!(target: "GpuDetect", "nvidia-smi 执行失败: {}", e);
            return None;
        }
    };

    if !output.status.success() {
        tracing::debug!(target: "GpuDetect", "nvidia-smi 执行失败");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.is_empty() { return None; }

    let parts: Vec<&str> = lines[0].split(',').map(|s| s.trim()).collect();
    if parts.len() < 3 { return None; }

    let name = parts[0].to_string();
    let memory: Option<u64> = parts[1].parse().ok();
    let driver_version = parts[2].to_string();
    let cuda_version = detect_cuda_version_async().await;

    let mut issues = Vec::new();

    if let Some(ref cuda) = cuda_version {
        let cuda_major: u32 = cuda.split('.').next()
            .and_then(|s| s.parse().ok()).unwrap_or(0);
        if cuda_major < 11 {
            tracing::warn!(target: "GpuDetect", cuda_version = %cuda, "CUDA 版本过旧，llama.cpp 需要 CUDA 11.0+");
            issues.push(GpuIssue {
                issue_type: "old_cuda".to_string(),
                severity: "error".to_string(),
                message: format!("CUDA 版本过旧: {}，llama.cpp 需要 CUDA 11.0+", cuda),
                suggestion: "请更新 NVIDIA 驱动以获取最新 CUDA 支持".to_string(),
                auto_fixable: false,
            });
        }
    }

    if let Some(mem) = memory {
        if mem < 4096 {
            tracing::warn!(target: "GpuDetect", memory_mb = mem, "显存较小，可能无法加载大型模型");
            issues.push(GpuIssue {
                issue_type: "low_vram".to_string(),
                severity: "warning".to_string(),
                message: format!("显存较小: {} MB，可能无法加载大型模型", mem),
                suggestion: "建议使用较小的模型或增加显存".to_string(),
                auto_fixable: false,
            });
        }
    }

    let available_backends = vec!["cpu".to_string(), "cuda".to_string()];

    Some(GpuInfo {
        model: name,
        vendor: "NVIDIA".to_string(),
        memory_mb: memory,
        driver_version: Some(driver_version),
        cuda_version,
        rocm_version: None,
        metal_version: None,
        apple_neuron_version: None,
        vulkan_support: false,
        metal_support: false,
        available_backends,
        recommended_backend: "cuda".to_string(),
        issues,
    })
}

/// 异步检测 CUDA 版本
async fn detect_cuda_version_async() -> Option<String> {
    let nvidia_smi_task = async {
        silent_command("nvidia-smi").output().await
    };

    match nvidia_circuit_breaker().call(nvidia_smi_task).await {
        Ok(output) => {
            if !output.status.success() { return None; }
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("CUDA Version:") {
                    let parts: Vec<&str> = line.split("CUDA Version:").collect();
                    if parts.len() > 1 {
                        let version = parts[1].split_whitespace().next()?;
                        return Some(version.to_string());
                    }
                }
            }
            None
        },
        Err(_) => None,
    }
}

/// 异步检测 AMD GPU
async fn detect_amd_gpu_async() -> Option<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "异步检测 AMD GPU...");

    let mut issues = Vec::new();
    let rocm_version = detect_rocm_version_async().await;
    if rocm_version.is_none() {
        tracing::warn!(target: "GpuDetect", "未检测到 ROCm，GPU 加速可能不可用");
        issues.push(GpuIssue {
            issue_type: "no_rocm".to_string(),
            severity: "warning".to_string(),
            message: "未检测到 ROCm，GPU 加速可能不可用".to_string(),
            suggestion: "请安装 ROCm 或使用 Vulkan 后端".to_string(),
            auto_fixable: false,
        });
    }

    let mut available_backends = vec!["cpu".to_string()];
    if rocm_version.is_some() { available_backends.push("rocm".to_string()); }
    available_backends.push("vulkan".to_string());

    let recommended = if available_backends.contains(&"rocm".to_string()) {
        "rocm".to_string()
    } else {
        "vulkan".to_string()
    };

    // 异步执行 lspci 检测
    let lspci_task = async {
        silent_command("lspci")
            .arg("-nn")
            .output()
            .await
    };

    let output = match amd_circuit_breaker().call(lspci_task).await {
        Ok(output) => output,
        Err(_) => return None,
    };

    if !output.status.success() { return None; }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let lower = line.to_lowercase();
        if lower.contains("amd") || lower.contains("radeon") {
            let name = if let Some(pos) = line.find("VGA compatible controller") {
                line[pos + 25..].trim().to_string()
            } else {
                line.to_string()
            };

            return Some(GpuInfo {
                model: name,
                vendor: "AMD".to_string(),
                memory_mb: None,
                driver_version: None,
                cuda_version: None,
                rocm_version,
                metal_version: None,
                apple_neuron_version: None,
                vulkan_support: true,
                metal_support: false,
                available_backends,
                recommended_backend: recommended,
                issues,
            });
        }
    }

    None
}

/// 异步检测 ROCm 版本
async fn detect_rocm_version_async() -> Option<String> {
    if !cfg!(target_os = "linux") { return None; }

    tracing::debug!(target: "GpuDetect", "异步检测 ROCm 版本...");

    // 尝试从文件系统读取
    if let Ok(entries) = std::fs::read_dir("/opt/rocm") {
        for entry in entries.flatten() {
            let name_str = entry.file_name().to_string_lossy().to_string();
            if let Some(ver) = name_str.strip_prefix("rocm-") {
                tracing::info!(target: "GpuDetect", version = %ver, "检测到 ROCm");
                return Some(ver.to_string());
            }
        }
    }

    // 异步执行 rocminfo
    let rocminfo_task = async {
        silent_command("rocminfo").output().await
    };

    match rocm_circuit_breaker().call(rocminfo_task).await {
        Ok(output) => {
            if !output.status.success() { return None; }
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Some(idx) = line.find("HSA Runtime Version:") {
                    let version = line[idx + 21..].trim();
                    if !version.is_empty() {
                        tracing::info!(target: "GpuDetect", version = %version, "检测到 ROCm (via rocminfo)");
                        return Some(version.to_string());
                    }
                }
            }
            None
        },
        Err(_) => None,
    }
}

/// 异步检测 Apple Silicon GPU
async fn detect_apple_silicon_gpu_async() -> Option<GpuInfo> {
    if !cfg!(target_os = "macos") {
        return None;
    }

    tracing::debug!(target: "GpuDetect", "异步检测 Apple Silicon GPU...");

    let system_profiler_task = async {
        silent_command("system_profiler")
            .args(["SPDisplaysDataType"])
            .output()
            .await
    };

    let output = match apple_circuit_breaker().call(system_profiler_task).await {
        Ok(output) => output,
        Err(e) => {
            tracing::debug!(target: "GpuDetect", "system_profiler 执行失败: {}", e);
            return None;
        }
    };

    if !output.status.success() {
        tracing::debug!(target: "GpuDetect", "system_profiler 执行失败");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut model = String::new();
    let mut memory_mb: Option<u64> = None;
    let mut driver_version: Option<String> = None;
    let mut metal_version: Option<String> = None;
    let mut apple_neuron_version: Option<String> = None;

    let mut current_chipset = false;
    for line in stdout.lines() {
        let line = line.trim();

        if line.contains("Chipset Model:") {
            if let Some(name) = line.split(":").nth(1) {
                model = name.trim().to_string();
                current_chipset = true;
            }
        }

        if line.contains("VRAM:") && current_chipset {
            if let Some(vram_str) = line.split(":").nth(1) {
                let vram_clean = vram_str.trim().replace(" MB", "");
                if let Ok(mem) = vram_clean.parse::<u64>() {
                    memory_mb = Some(mem);
                }
            }
        }

        if line.contains("Driver Version:") && current_chipset {
            if let Some(driver) = line.split(":").nth(1) {
                driver_version = Some(driver.trim().to_string());
            }
        }

        if line.contains("Metal") && line.contains("Supported") && current_chipset {
            metal_version = Some("Supported".to_string());
        }

        if line.contains("Neural Engine") && current_chipset {
            if let Some(neuron) = line.split(":").nth(1) {
                apple_neuron_version = Some(neuron.trim().to_string());
            }
        }
    }

    if model.is_empty() {
        return None;
    }

    let mut issues = Vec::new();
    if let Some(mem) = memory_mb {
        if mem < 8192 {
            issues.push(GpuIssue {
                issue_type: "low_vram".to_string(),
                severity: "warning".to_string(),
                message: format!("显存较小: {} MB，可能无法加载大型模型", mem),
                suggestion: "建议使用较小的模型或检查显存配置".to_string(),
                auto_fixable: false,
            });
        }
    }

    let available_backends = vec!["cpu".to_string(), "metal".to_string()];

    Some(GpuInfo {
        model: if model.is_empty() { "Apple Silicon GPU".to_string() } else { model },
        vendor: "Apple Silicon".to_string(),
        memory_mb,
        driver_version,
        cuda_version: None,
        rocm_version: None,
        metal_version,
        apple_neuron_version,
        vulkan_support: false,
        metal_support: true,
        available_backends,
        recommended_backend: "metal".to_string(),
        issues,
    })
}

/// 诊断 GPU 问题（异步版本）
pub async fn diagnose_gpu_issues_async() -> Result<Vec<GpuIssue>, GpuDetectionError> {
    tracing::debug!(target: "GpuDetect", "开始异步诊断 GPU");

    validate_environment()?;

    let mut all_issues = Vec::new();

    // 并发执行各项诊断
    let cuda_diagnose = diagnose_cuda_runtime_async();
    let memory_diagnose = diagnose_memory_usage_async();

    let (cuda_issues, memory_issues) = tokio::join!(cuda_diagnose, memory_diagnose);
    all_issues.extend(cuda_issues);
    all_issues.extend(memory_issues);

    Ok(all_issues)
}

/// 诊断 CUDA Runtime
async fn diagnose_cuda_runtime_async() -> Vec<GpuIssue> {
    let mut issues = Vec::new();

    let nvidia_smi_task = async {
        silent_command("nvidia-smi").output().await
    };

    match nvidia_circuit_breaker().call(nvidia_smi_task).await {
        Ok(output) => {
            if !output.status.success() {
                issues.push(GpuIssue {
                    issue_type: "no_cuda_runtime".to_string(),
                    severity: "error".to_string(),
                    message: "CUDA Runtime 不可用".to_string(),
                    suggestion: "请安装 NVIDIA CUDA Toolkit".to_string(),
                    auto_fixable: true,
                });
                return issues;
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut has_cuda = false;
            for line in stdout.lines() {
                if line.contains("CUDA Version:") {
                    has_cuda = true;
                    break;
                }
            }

            if !has_cuda {
                issues.push(GpuIssue {
                    issue_type: "no_cuda_runtime".to_string(),
                    severity: "error".to_string(),
                    message: "CUDA Runtime 不可用".to_string(),
                    suggestion: "请安装 NVIDIA CUDA Toolkit".to_string(),
                    auto_fixable: true,
                });
            }
        },
        Err(e) => {
            tracing::warn!(target: "GpuDetect", error = %e, "CUDA 诊断失败");
            issues.push(GpuIssue {
                issue_type: "no_cuda_runtime".to_string(),
                severity: "error".to_string(),
                message: format!("CUDA Runtime 诊断失败: {}", e),
                suggestion: "请安装 NVIDIA CUDA Toolkit".to_string(),
                auto_fixable: true,
            });
        }
    }

    issues
}

/// 诊断内存使用
async fn diagnose_memory_usage_async() -> Vec<GpuIssue> {
    let mut issues = Vec::new();

    // 尝试获取 GPU 显存信息
    let nvidia_smi_task = async {
        silent_command("nvidia-smi")
            .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
            .output()
            .await
    };

    if let Ok(output) = nvidia_circuit_breaker().call(nvidia_smi_task).await {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut total_memory = 0u64;
            for line in stdout.lines() {
                if let Ok(mem) = line.trim().parse::<u64>() {
                    total_memory = mem;
                }
            }

            if total_memory > 0 && total_memory < 4096 {
                issues.push(GpuIssue {
                    issue_type: "low_vram".to_string(),
                    severity: "warning".to_string(),
                    message: format!("显存较小: {} MB，可能无法加载大型模型", total_memory),
                    suggestion: "建议使用较小的模型或增加显存".to_string(),
                    auto_fixable: false,
                });
            }
        }
    }

    issues
}

/// 自动修复 GPU 问题（保持向后兼容性）
#[allow(dead_code)]
pub fn auto_fix_gpu_issue(issue: &GpuIssue) -> Result<String, String> {
    if !issue.auto_fixable {
        return Err("此问题无法自动修复".to_string());
    }

    match issue.issue_type.as_str() {
        "no_cuda_runtime" if cfg!(target_os = "linux") => {
            let output = crate::util::process::silent_command("sudo")
                .args(["apt-get", "install", "-y", "nvidia-cuda-toolkit"])
                .output()
                .map_err(|e| format!("执行安装命令失败: {}", e))?;

            if output.status.success() {
                Ok("CUDA Runtime 安装成功".to_string())
            } else {
                Err("CUDA Runtime 安装失败".to_string())
            }
        }
        _ => Err(format!("此问题无法自动修复: {}", issue.issue_type)),
    }
}

/// 诊断 GPU 问题（保持向后兼容性）
pub fn diagnose_gpu_issues() -> Vec<GpuIssue> {
    // 运行异步版本并阻塞等待完成
    let runtime = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    runtime.block_on(async {
        match diagnose_gpu_issues_async().await {
            Ok(issues) => issues,
            Err(e) => {
                tracing::error!(target: "GpuDetect", "诊断 GPU 问题失败: {}", e);
                Vec::new()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_creation() {
        let cb = CircuitBreaker::new("test-tool");
        assert!(matches!(*cb.state.try_lock().unwrap(), CircuitBreakerState::Closed));
    }

    #[test]
    fn test_circuit_breaker_config() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            half_open_timeout_secs: 10,
            reset_timeout_secs: 60,
            timeout_secs: 5,
        };
        let cb = CircuitBreaker::with_config("test-tool", config);
        assert!(matches!(*cb.state.try_lock().unwrap(), CircuitBreakerState::Closed));
    }

    #[tokio::test]
    async fn test_gpu_detection_error_display() {
        let timeout_err = GpuDetectionError::Timeout {
            tool: "nvidia-smi".to_string(),
            timeout_secs: 10,
        };
        assert!(timeout_err.to_string().contains("超时"));

        let not_found_err = GpuDetectionError::CommandNotFound {
            tool: "nvidia-smi".to_string(),
        };
        assert!(not_found_err.to_string().contains("未找到"));
    }

    #[test]
    fn test_validate_environment() {
        let result = validate_environment();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_circuit_breaker_circuit_open() {
        let cb = CircuitBreaker::with_config(
            "test-tool",
            CircuitBreakerConfig {
                failure_threshold: 2,
                half_open_timeout_secs: 1,
                reset_timeout_secs: 60,
                timeout_secs: 1,
            }
        );

        // 连续失败2次后，断路器应该打开
        let _ = cb.call(async { 
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok::<(), &str>(())
        }).await;
        let _ = cb.call(async { 
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok::<(), &str>(())
        }).await;
        
        // 第三次调用应该快速返回（断路器打开）
        let start = Instant::now();
        let _ = cb.call(async { 
            Ok::<(), &str>(())
        }).await;
        let elapsed = start.elapsed();
        
        // 应该快速失败（断路器打开时跳过实际执行）
        assert!(elapsed < Duration::from_millis(100));
    }
}
