//! 平台与 GPU 后端检测。
//!
//! 提供：
//! - 操作系统检测
//! - CPU 架构检测
//! - GPU 后端检测（自动选择最合适的 llama.cpp 构建）

#![allow(clippy::module_name_repetitions)]

/// GPU 后端类型
///
/// 命名与 llama.cpp 官方 Release 资产名保持一致（`as_str()` 的输出
/// 直接参与资产名匹配），因此修改变体名或字符串值会破坏下载匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuBackend {
    /// 纯 CPU 推理
    Cpu,
    /// CUDA 12.4 兼容模式（同时兼容 12.x 驱动）
    Cuda12_4,
    /// CUDA 13.3+
    Cuda13_3,
    /// AMD ROCm
    Rocm,
    /// Vulkan
    Vulkan,
    /// Apple Metal
    Metal,
}

impl GpuBackend {
    /// 转换为 llama.cpp Release 资产名中的后端标识符
    ///
    /// 该字符串会被拼进 `llama-<backend>-bin-*.zip` 的匹配模式，
    /// 修改会导致线上下载 404。
    pub const fn as_str(&self) -> &'static str {
        match self {
            GpuBackend::Cpu => "cpu",
            GpuBackend::Cuda12_4 => "cuda-12.4",
            GpuBackend::Cuda13_3 => "cuda-13.3",
            GpuBackend::Rocm => "rocm",
            GpuBackend::Vulkan => "vulkan",
            GpuBackend::Metal => "metal",
        }
    }

    /// 从用户输入 / 环境变量解析 GPU 后端
    ///
    /// 采用「宽容解析」策略：无法识别的输入一律降级为 [`GpuBackend::Cpu`]，
    /// 而不是返回错误。这样配置里的手写字符串不会让整个应用启动失败。
    /// 需要严格校验的场景请显式比对 [`GpuBackend::as_str`]。
    pub fn parse_backend(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "cuda" | "cuda12" | "cuda12_4" | "cuda-12.4" => GpuBackend::Cuda12_4,
            "cuda13" | "cuda13_3" | "cuda-13.3" => GpuBackend::Cuda13_3,
            "rocm" => GpuBackend::Rocm,
            "vulkan" => GpuBackend::Vulkan,
            "metal" => GpuBackend::Metal,
            _ => GpuBackend::Cpu,
        }
    }

    /// 该后端是否需要 CUDA / ROCm 运行时（纯 CPU 与 Vulkan 不需要）
    pub const fn requires_vendor_runtime(&self) -> bool {
        matches!(
            self,
            GpuBackend::Cuda12_4 | GpuBackend::Cuda13_3 | GpuBackend::Rocm
        )
    }
}

/// 严格解析：无法识别时返回错误（供 CLI / 配置校验使用）
impl std::str::FromStr for GpuBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cpu" => Ok(GpuBackend::Cpu),
            "cuda" | "cuda12" | "cuda12_4" | "cuda-12.4" => Ok(GpuBackend::Cuda12_4),
            "cuda13" | "cuda13_3" | "cuda-13.3" => Ok(GpuBackend::Cuda13_3),
            "rocm" => Ok(GpuBackend::Rocm),
            "vulkan" => Ok(GpuBackend::Vulkan),
            "metal" => Ok(GpuBackend::Metal),
            other => Err(format!(
                "未知 GPU 后端 `{other}`（可选值：cpu / cuda-12.4 / cuda-13.3 / rocm / vulkan / metal）"
            )),
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
            return GpuBackend::Cuda12_4;
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
            return GpuBackend::Cuda12_4;
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
        "windows" => vec![GpuBackend::Cuda12_4, GpuBackend::Vulkan, GpuBackend::Cpu],
        "linux" => vec![
            GpuBackend::Cuda12_4,
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
        assert_eq!(
            GpuBackend::from_str("cuda-12.4").unwrap(),
            GpuBackend::Cuda12_4
        );
        assert_eq!(GpuBackend::from_str("CPU").unwrap(), GpuBackend::Cpu);
    }

    #[test]
    fn gpu_backend_from_str_rejects_unknown() {
        use std::str::FromStr;
        assert!(GpuBackend::from_str("nonexistent").is_err());
    }

    #[test]
    fn gpu_backend_str() {
        assert_eq!(GpuBackend::Cuda12_4.as_str(), "cuda-12.4");
        assert_eq!(GpuBackend::Metal.as_str(), "metal");
    }

    #[test]
    fn parse_backend_is_lenient() {
        assert_eq!(GpuBackend::parse_backend("cuda13"), GpuBackend::Cuda13_3);
        assert_eq!(GpuBackend::parse_backend("garbage"), GpuBackend::Cpu);
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
