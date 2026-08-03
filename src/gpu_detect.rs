//! GPU 检测与诊断模块。
//!
//! 检测系统 GPU 型号、驱动版本、CUDA/ROCm/Vulkan 版本，
//! 并提供自动修复建议。

use serde::{Deserialize, Serialize};
use std::process::Command;

/// GPU 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// GPU 型号
    pub model: String,
    /// 制造商 (NVIDIA/AMD/Intel/Unknown)
    pub vendor: String,
    /// 显存大小 (MB)
    pub memory_mb: Option<u64>,
    /// 驱动版本
    pub driver_version: Option<String>,
    /// CUDA 版本（NVIDIA）
    pub cuda_version: Option<String>,
    /// ROCm 版本（AMD）
    pub rocm_version: Option<String>,
    /// Vulkan 支持
    pub vulkan_support: bool,
    /// Metal 支持（macOS）
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

/// 检测所有 GPU
pub fn detect_all_gpus() -> Vec<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "开始检测所有 GPU");

    let mut gpus = Vec::new();

    if let Some(nvidia) = detect_nvidia_gpu() {
        gpus.push(nvidia);
    }
    if let Some(amd) = detect_amd_gpu() {
        gpus.push(amd);
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
            vulkan_support: false,
            metal_support: false,
            available_backends: vec!["cpu".to_string()],
            recommended_backend: "cpu".to_string(),
            issues: vec![GpuIssue {
                issue_type: "no_gpu".to_string(),
                severity: "info".to_string(),
                message: "未检测到独立 GPU，将使用 CPU 推理".to_string(),
                suggestion: "如需 GPU 加速，请安装 NVIDIA/AMD 独立显卡".to_string(),
                auto_fixable: false,
            }],
        });
    } else {
        for gpu in &gpus {
            tracing::info!(
                target: "GpuDetect",
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = ?gpu.memory_mb,
                driver = ?gpu.driver_version,
                cuda = ?gpu.cuda_version,
                recommended = %gpu.recommended_backend,
                "检测到 GPU"
            );
        }
    }

    gpus
}

/// 检测 NVIDIA GPU
fn detect_nvidia_gpu() -> Option<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "检测 NVIDIA GPU...");

    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;

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
    let cuda_version = detect_cuda_version();

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

    // nvidia-smi 查询成功即证明 NVIDIA 驱动已安装，CUDA 后端可用。
    // 不应依赖 CUDA 版本字符串解析结果来决定是否推荐 CUDA。
    let mut available_backends = vec!["cpu".to_string(), "cuda".to_string()];

    Some(GpuInfo {
        model: name,
        vendor: "NVIDIA".to_string(),
        memory_mb: memory,
        driver_version: Some(driver_version),
        cuda_version,
        rocm_version: None,
        vulkan_support: false,
        metal_support: false,
        available_backends,
        recommended_backend: "cuda".to_string(),
        issues,
    })
}

/// 检测 CUDA 版本
fn detect_cuda_version() -> Option<String> {
    let output = Command::new("nvidia-smi").output().ok()?;
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
}

/// 检测 AMD GPU
fn detect_amd_gpu() -> Option<GpuInfo> {
    tracing::debug!(target: "GpuDetect", "检测 AMD GPU...");

    // Windows: 通过 WMI 检测
    if cfg!(target_os = "windows") {
        let output = Command::new("wmic")
            .args(["path", "win32_videocontroller", "get", "name"])
            .output()
            .ok()?;

        if !output.status.success() { return None; }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let lower = line.to_lowercase();
            if lower.contains("amd") || lower.contains("radeon") {
                let name = line.trim().to_string();
                tracing::info!(target: "GpuDetect", model = %name, "检测到 AMD GPU");
                return Some(GpuInfo {
                    model: name,
                    vendor: "AMD".to_string(),
                    memory_mb: None,
                    driver_version: None,
                    cuda_version: None,
                    rocm_version: None,
                    vulkan_support: true,
                    metal_support: false,
                    available_backends: vec!["cpu".to_string(), "vulkan".to_string()],
                    recommended_backend: "vulkan".to_string(),
                    issues: vec![],
                });
            }
        }
        return None;
    }

    // Linux: 通过 lspci 检测
    if cfg!(target_os = "linux") {
        let output = Command::new("lspci")
            .arg("-nn")
            .output()
            .ok()?;

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

                let mut issues = Vec::new();
                let rocm_version = detect_rocm_version();
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

                return Some(GpuInfo {
                    model: name,
                    vendor: "AMD".to_string(),
                    memory_mb: None,
                    driver_version: None,
                    cuda_version: None,
                    rocm_version,
                    vulkan_support: true,
                    metal_support: false,
                    available_backends,
                    recommended_backend: recommended,
                    issues,
                });
            }
        }
    }

    None
}

/// 检测 ROCm 版本
fn detect_rocm_version() -> Option<String> {
    // Linux only
    if !cfg!(target_os = "linux") { return None; }

    tracing::debug!(target: "GpuDetect", "检测 ROCm 版本...");

    if let Ok(entries) = std::fs::read_dir("/opt/rocm") {
        for entry in entries.flatten() {
            let name_str = entry.file_name().to_string_lossy().to_string();
            if let Some(ver) = name_str.strip_prefix("rocm-") {
                tracing::info!(target: "GpuDetect", version = %ver, "检测到 ROCm");
                return Some(ver.to_string());
            }
        }
    }

    if let Ok(output) = Command::new("rocminfo").output() {
        if output.status.success() {
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
        }
    }

    None
}

/// 诊断 GPU 问题并提供修复建议
pub fn diagnose_gpu_issues() -> Vec<GpuIssue> {
    tracing::debug!(target: "GpuDetect", "开始 GPU 诊断");
    let gpus = detect_all_gpus();
    let mut all_issues: Vec<GpuIssue> = gpus.iter().flat_map(|g| g.issues.clone()).collect();

    // 检查 Vulkan 支持
    let has_vulkan = gpus.iter().any(|g| g.vulkan_support);
    if !has_vulkan {
        let vulkan_ok = Command::new("vulkaninfo")
            .arg("--summary")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !vulkan_ok {
            all_issues.push(GpuIssue {
                issue_type: "no_vulkan".to_string(),
                severity: "info".to_string(),
                message: "未检测到 Vulkan 支持".to_string(),
                suggestion: "安装 mesa-vulkan-drivers (Linux) 或 Vulkan SDK (Windows)".to_string(),
                auto_fixable: false,
            });
        }
    }

    tracing::info!(target: "GpuDetect", issue_count = all_issues.len(), "GPU 诊断完成");
    all_issues
}

/// 自动修复 GPU 问题
#[allow(dead_code)]
pub fn auto_fix_gpu_issue(issue: &GpuIssue) -> Result<String, String> {
    if !issue.auto_fixable {
        return Err("此问题无法自动修复".to_string());
    }

    match issue.issue_type.as_str() {
        "no_cuda_runtime" if cfg!(target_os = "linux") => {
            let output = Command::new("sudo")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_all_gpus() {
        let gpus = detect_all_gpus();
        assert!(!gpus.is_empty());
    }

    #[test]
    fn test_diagnose_gpu_issues() {
        let _issues = diagnose_gpu_issues();
    }
}
