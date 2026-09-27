//! 平台与 GPU 后端检测。
//!
//! 提供：
//! - 操作系统检测
//! - CPU 架构检测
//! - GPU 后端检测（自动选择最合适的 llama.cpp 构建）

#![allow(dead_code)]

/// GPU 后端类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuBackend {
    Cpu,
    Cuda,
    Vulkan,
    Metal,
    Rocm,
    Sycl,
    Blas,
}

impl GpuBackend {
    /// 转换为字符串标识符
    pub fn as_str(&self) -> &'static str {
        match self {
            GpuBackend::Cpu => "cpu",
            GpuBackend::Cuda => "cuda",
            GpuBackend::Vulkan => "vulkan",
            GpuBackend::Metal => "metal",
            GpuBackend::Rocm => "rocm",
            GpuBackend::Sycl => "sycl",
            GpuBackend::Blas => "blas",
        }
    }
}

/// 从字符串解析 GPU 后端
impl std::str::FromStr for GpuBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "cpu" => Ok(GpuBackend::Cpu),
            "cuda" => Ok(GpuBackend::Cuda),
            "vulkan" => Ok(GpuBackend::Vulkan),
            "metal" => Ok(GpuBackend::Metal),
            "rocm" => Ok(GpuBackend::Rocm),
            "sycl" => Ok(GpuBackend::Sycl),
            "blas" => Ok(GpuBackend::Blas),
            _ => Err(format!("未知 GPU 后端: {}", s)),
        }
    }
}

/// 当前操作系统
pub fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    }
}

/// 当前 CPU 架构
pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    }
}

/// 检测最佳 GPU 后端
pub fn detect_gpu_backend() -> GpuBackend {
    // Windows: 优先 CUDA，其次 Vulkan，最后 CPU
    if cfg!(target_os = "windows") {
        if std::path::Path::new("C:/Program Files/NVIDIA Corporation").is_dir() {
            return GpuBackend::Cuda;
        }
        return GpuBackend::Cpu;
    }

    // macOS: 始终使用 Metal
    if cfg!(target_os = "macos") {
        return GpuBackend::Metal;
    }

    // Linux: 优先 CUDA，其次 ROCm，最后 Vulkan/CPU
    if cfg!(target_os = "linux") {
        if std::path::Path::new("/dev/nvidiactl").exists() {
            return GpuBackend::Cuda;
        }
        if std::path::Path::new("/sys/class/kfd").exists() {
            return GpuBackend::Rocm;
        }
        return GpuBackend::Cpu;
    }

    GpuBackend::Cpu
}

/// 获取当前平台可用的 GPU 后端列表
pub fn available_backends() -> Vec<GpuBackend> {
    let os = current_os();
    match os {
        "windows" => vec![GpuBackend::Cuda, GpuBackend::Vulkan, GpuBackend::Cpu],
        "linux" => vec![
            GpuBackend::Cuda,
            GpuBackend::Rocm,
            GpuBackend::Vulkan,
            GpuBackend::Cpu,
        ],
        "macos" => vec![GpuBackend::Metal, GpuBackend::Cpu],
        _ => vec![GpuBackend::Cpu],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_os_not_empty() {
        assert!(!current_os().is_empty());
    }

    #[test]
    fn current_arch_not_empty() {
        assert!(!current_arch().is_empty());
    }

    #[test]
    fn gpu_backend_from_str() {
        use std::str::FromStr;
        assert_eq!(GpuBackend::from_str("cuda").unwrap(), GpuBackend::Cuda);
        assert_eq!(GpuBackend::from_str("CPU").unwrap(), GpuBackend::Cpu);
    }

    #[test]
    fn gpu_backend_str() {
        assert_eq!(GpuBackend::Cuda.as_str(), "cuda");
        assert_eq!(GpuBackend::Metal.as_str(), "metal");
    }

    #[test]
    fn detect_gpu_not_empty() {
        let backend = detect_gpu_backend();
        assert!(!backend.as_str().is_empty());
    }

    #[test]
    fn available_backends_include_cpu() {
        let backends = available_backends();
        assert!(backends.contains(&GpuBackend::Cpu));
    }
}