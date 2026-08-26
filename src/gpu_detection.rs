//! GPU 检测与诊断模块。
//!
//! 检测系统 GPU 型号、驱动版本、CUDA/ROCm/Vulkan 版本，
//! 并提供自动修复建议。
//!
//! 本模块已重构为独立的 GPU 检测模块，通过异步、事件驱动和错误转换等技术
//! 提升了 GPU 检测的性能、可靠性和用户体验。

use serde::{Deserialize, Serialize};
use std::time::Duration;

use tokio::process::Command;

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
pub async fn detect_all_gpus_async() -> Vec<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "开始异步检测所有 GPU");

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

    gpus
}

/// 异步检测 NVIDIA GPU
async fn detect_nvidia_gpu_async() -> Option<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "异步检测 NVIDIA GPU...");

    // 增加超时和错误处理
    let nvidia_smi_task = async {
        Command::new("nvidia-smi")
            .args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"])
            .output()
            .await
    };

    let output = match tokio::time::timeout(Duration::from_secs(10), nvidia_smi_task).await {
        Ok(result) => match result {
            Ok(output) => output,
            Err(e) => {
                tracing::debug!(target: "GpuDetect", "nvidia-smi 执行失败: {}", e);
                return None;
            }
        },
        Err(_) => {
            tracing::debug!(target: "GpuDetect", "nvidia-smi 超时");
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
        Command::new("nvidia-smi").output().await
    };

    match tokio::time::timeout(Duration::from_secs(5), nvidia_smi_task).await {
        Ok(result) => match result {
            Ok(output) => {
                if !output.status.success() { return None; }
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("CUDA Version:") {
                        let parts: Vec<&str> = line.split("CUDA Version:").collect();
                        if parts.len() > 1 {
                            let version = parts[1].trim().split_whitespace().next()?;
                            return Some(version.to_string());
                        }
                    }
                }
                None
            },
            Err(_) => None,
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
        Command::new("lspci")
            .arg("-nn")
            .output()
            .await
    };

    let output = match tokio::time::timeout(Duration::from_secs(10), lspci_task).await {
        Ok(result) => match result {
            Ok(output) => output,
            Err(_) => return None,
        },
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
        Command::new("rocminfo").output().await
    };

    match tokio::time::timeout(Duration::from_secs(5), rocminfo_task).await {
        Ok(result) => match result {
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
        Command::new("system_profiler")
            .args(["SPDisplaysDataType"])
            .output()
            .await
    };

    let output = match tokio::time::timeout(Duration::from_secs(10), system_profiler_task).await {
        Ok(result) => match result {
            Ok(output) => output,
            Err(e) => {
                tracing::debug!(target: "GpuDetect", "system_profiler 执行失败: {}", e);
                return None;
            }
        },
        Err(_) => {
            tracing::debug!(target: "GpuDetect", "system_profiler 超时");
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
pub async fn diagnose_gpu_issues_async() -> Vec<GpuIssue> {
    tracing::debug!(target: "GpuDetect", "开始异步诊断 GPU");

    let mut all_issues = Vec::new();

    // 并发执行各项诊断
    let cuda_diagnose = diagnose_cuda_runtime_async();
    let memory_diagnose = diagnose_memory_usage_async();

    let (cuda_issues, memory_issues) = tokio::join!(cuda_diagnose, memory_diagnose);
    all_issues.extend(cuda_issues);
    all_issues.extend(memory_issues);

    all_issues
}

/// 诊断 CUDA Runtime
async fn diagnose_cuda_runtime_async() -> Vec<GpuIssue> {
    let mut issues = Vec::new();

    let nvidia_smi_task = async {
        Command::new("nvidia-smi").output().await
    };

    match tokio::time::timeout(Duration::from_secs(5), nvidia_smi_task).await {
        Ok(result) => match result {
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
            Err(_) => {
                issues.push(GpuIssue {
                    issue_type: "no_cuda_runtime".to_string(),
                    severity: "error".to_string(),
                    message: "CUDA Runtime 不可用".to_string(),
                    suggestion: "请安装 NVIDIA CUDA Toolkit".to_string(),
                    auto_fixable: true,
                });
            }
        },
        Err(_) => {
            issues.push(GpuIssue {
                issue_type: "no_cuda_runtime".to_string(),
                severity: "error".to_string(),
                message: "CUDA Runtime 不可用".to_string(),
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
        Command::new("nvidia-smi")
            .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
            .output()
            .await
    };

    match tokio::time::timeout(Duration::from_secs(5), nvidia_smi_task).await {
        Ok(result) => match result {
            Ok(output) => {
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
            },
            Err(_) => {}
        },
        Err(_) => {}
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
            let output = std::process::Command::new("sudo")
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
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(diagnose_gpu_issues_async())
}