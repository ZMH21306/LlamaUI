// Configuration persistence module
// Reads/writes user config (model path, parameters, llama-server path) to a JSON file.
//
// 重构说明：
//   - validate() 改用 `crate::error::ConfigError` 作为返回类型，提供更精确的错误分类。
//   - 默认 ConfigStore::set() 仍返回 anyhow::Result 以兼容既有调用方，
//     内部转换 ConfigError → anyhow::Error。

use crate::error::ConfigError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::Mutex;

const CONFIG_FILE: &str = "config.json";

/// 配置 schema 版本号。每次破坏性变更（重命名字段、改变语义、移除字段）必须 +1。
/// 旧版本文件会在 `load_from_disk` 阶段被静默迁移到当前版本，迁移失败时退回默认。
pub const CURRENT_CONFIG_VERSION: u32 = 1;

/// 专业模式默认启动命令（首次安装或新配置时使用）
pub const DEFAULT_PRO_CUSTOM_COMMAND: &str =
    "\"%%llama_server%%\" --models-dir \"%%models_dir%%\" --host %%host%% --port %%port%% -ngl all -c 32768 -fa on -ctk q5_0 -ctv q5_0 --spec-type draft-mtp --spec-draft-n-max 3 -tb 32";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Schema 版本。读盘时校验，旧版本会被迁移。手动写入新字段不需要
    /// 关心，但删除/重命名字段时需在 `migrate` 中加分支。
    #[serde(default = "default_config_version")]
    pub _v: u32,
    /// llama-server 可执行文件路径（None 表示从 PATH 自动检测）
    pub llama_server_path: Option<String>,
    /// 包含所有 .gguf 模型文件的目录（路由模式）
    pub models_dir: String,
    /// 上下文大小（默认 8192，范围 128~1048576）
    pub ctx_size: u32,
    /// GPU 卸载层数（-1 = 全部，0 = 不使用，n = 指定层数，范围 -1~200）
    pub n_gpu_layers: i32,
    /// 是否启用 Flash Attention
    pub flash_attn: bool,
    /// 是否启用 MTP 多 token 预测
    pub mtp: bool,
    /// MTP 草稿数量（--spec-draft-n-max，范围 0~16）
    pub mtp_draft_n_max: u32,
    /// HTTP 端口（若被占用，根据 auto_port 自动切换，范围 1~65535）
    pub port: u16,
    /// 端口被占用时是否自动寻找空闲端口
    pub auto_port: bool,
    /// 额外参数（追加到命令末尾的自由文本）
    pub extra_args: String,
    /// 参数模式："normal" | "advanced" | "pro"
    #[serde(default = "default_mode")]
    pub mode: String,
    /// 专业模式：完整启动命令（含程序名 + 参数）。
    /// 启动时按空白拆分为 argv，并将 `%%var%%` 形式的变量替换为实际值。
    /// 留空时回退到普通模式命令。
    #[serde(default)]
    pub custom_command: String,
}

fn default_mode() -> String {
    "normal".to_string()
}

fn default_config_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            _v: CURRENT_CONFIG_VERSION,
            llama_server_path: None,
            models_dir: String::new(),
            ctx_size: 8192,
            n_gpu_layers: -1,
            flash_attn: true,
            mtp: true,
            mtp_draft_n_max: 3,
            port: 10897,
            auto_port: true,
            extra_args: String::new(),
            mode: default_mode(),
            custom_command: DEFAULT_PRO_CUSTOM_COMMAND.to_string(),
        }
    }
}

impl AppConfig {
    /// 校验配置合法性。返回第一个失败的错误，所有规则在失败时立即返回。
    ///
    /// 错误类型为 `ConfigError`，提供精确分类（端口越界、模式非法、NUL 字符、
    /// 路径不存在等），上层 `ConfigStore::set` 会包装成 `anyhow::Error`。
    ///
    /// 规则：
    /// - `port` ∈ `[1, 65535]`
    /// - `mode` ∈ `{"normal", "advanced", "pro"}`
    /// - `ctx_size` ∈ `[128, 1_048_576]`（128 ~ 1M tokens）
    /// - `n_gpu_layers` ∈ `[-1, 200]`
    /// - `mtp_draft_n_max` ∈ `[0, 16]`
    /// - 若 `llama_server_path` 非空，必须指向存在的文件
    /// - 若 `models_dir` 非空，必须指向存在的目录
    /// - 路径中不能含 NUL 字符（Windows 路径非法）
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::PortZero);
        }
        match self.mode.as_str() {
            "normal" | "advanced" | "pro" => {}
            other => return Err(ConfigError::InvalidMode(other.to_string())),
        }
        if !(128..=1_048_576).contains(&self.ctx_size) {
            return Err(ConfigError::CtxSizeOutOfRange { value: self.ctx_size });
        }
        if !(-1..=200).contains(&self.n_gpu_layers) {
            return Err(ConfigError::GpuLayersOutOfRange { value: self.n_gpu_layers });
        }
        if self.mtp_draft_n_max > 16 {
            return Err(ConfigError::MtpDraftOutOfRange {
                value: self.mtp_draft_n_max,
            });
        }
        if self.custom_command.contains('\0') {
            return Err(ConfigError::NulInPath {
                field: "custom_command",
            });
        }
        if self.extra_args.contains('\0') {
            return Err(ConfigError::NulInPath {
                field: "extra_args",
            });
        }
        if let Some(p) = &self.llama_server_path {
            if !p.is_empty() {
                if p.contains('\0') {
                    return Err(ConfigError::NulInPath {
                        field: "llama_server_path",
                    });
                }
                let pb = std::path::Path::new(p);
                if !pb.exists() {
                    return Err(ConfigError::PathNotFound(pb.to_path_buf()));
                }
                if !pb.is_file() {
                    return Err(ConfigError::NotAFile(pb.to_path_buf()));
                }
            }
        }
        if !self.models_dir.is_empty() {
            if self.models_dir.contains('\0') {
                return Err(ConfigError::NulInPath {
                    field: "models_dir",
                });
            }
            let pb = std::path::Path::new(&self.models_dir);
            if !pb.exists() {
                return Err(ConfigError::PathNotFound(pb.to_path_buf()));
            }
            if !pb.is_dir() {
                return Err(ConfigError::NotADirectory(pb.to_path_buf()));
            }
        }
        Ok(())
    }
}

pub struct ConfigStore {
    inner: Arc<Mutex<AppConfig>>,
    path: PathBuf,
}

impl ConfigStore {
    pub fn new() -> Self {
        let path = config_path();
        let mut cfg = load_from_disk(&path).unwrap_or_default();
        // 一次性迁移：旧默认端口 8000 → 新默认 10897（只在用户从未改过且无现存服务时静默迁移）
        if cfg.port == 8000 {
            cfg.port = 10897;
            // 立即写回磁盘（首次迁移后再读就是新值）
            let _ = save_to_disk(&path, &cfg);
        }
        // 一次性迁移：旧的空 custom_command → 新的默认专业模式启动命令
        if cfg.custom_command.is_empty() {
            cfg.custom_command = DEFAULT_PRO_CUSTOM_COMMAND.to_string();
            let _ = save_to_disk(&path, &cfg);
        }
        Self {
            inner: Arc::new(Mutex::new(cfg)),
            path,
        }
    }

    pub fn get(&self) -> AppConfig {
        self.inner.lock().clone()
    }

    pub fn set(&self, new_cfg: AppConfig) -> anyhow::Result<()> {
        // 写入前先校验，避免持久化非法值（如 port=0、不存在的路径等）
        new_cfg.validate()?;
        // 写入时强制 bump 到当前 schema 版本
        let mut new_cfg = new_cfg;
        new_cfg._v = CURRENT_CONFIG_VERSION;
        {
            let mut guard = self.inner.lock();
            *guard = new_cfg.clone();
        }
        save_to_disk(&self.path, &new_cfg)
    }
}

fn config_path() -> PathBuf {
    // Use dirs::config_dir() to put config in user's config folder (cross-platform)
    let base = dirs::config_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    base.join("LlamaUI").join(CONFIG_FILE)
}

fn load_from_disk(path: &PathBuf) -> Option<AppConfig> {
    let data = fs::read_to_string(path).ok()?;
    let mut cfg: AppConfig = serde_json::from_str(&data).ok()?;
    // 旧配置 schema 迁移
    if cfg._v != CURRENT_CONFIG_VERSION {
        cfg = migrate(cfg);
        // 迁移后立即写回，避免下次再触发迁移
        let _ = save_to_disk(path, &cfg);
    }
    Some(cfg)
}

/// 旧 schema → 新 schema 的显式迁移。
///
/// 设计（P2-2 修复）：
/// - 使用 `match cfg._v` 显式分版本升级，未来新增 v2/v3 时只需追加分支。
/// - 每个分支只做"从 vN 升级到 v(N+1)"的最小变更，下一次循环会处理剩余的跨度。
/// - 升级结束后用 `cfg._v = CURRENT_CONFIG_VERSION` 一次性 bump，
///   避免在每个分支内重复设置版本号（少一处遗忘）。
/// - 不打印日志：迁移在启动期执行，没有 UI 上下文。
fn migrate(mut cfg: AppConfig) -> AppConfig {
    loop {
        match cfg._v {
            0 => {
                // v0 → v1：v0 没有 _v 字段（serde 用 default 填充为 0），
                // 升级到 v1：什么都不用做（仅占位，证明迁移链可工作）。
                cfg._v = 1;
            }
            1 => {
                // v1 → v2：当前与最新版本一致，跳出循环。
                break;
            }
            other => {
                // 未知版本（> CURRENT_CONFIG_VERSION）：可能是降级安装或外部
                // 篡改。保守策略是保留数据，仅对齐到当前版本号。
                // 用户的真实数据不会被覆盖。
                let _ = other;
                break;
            }
        }
    }
    cfg._v = CURRENT_CONFIG_VERSION;
    cfg
}

fn save_to_disk(path: &PathBuf, cfg: &AppConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(cfg)?;
    fs::write(path, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! 配置校验单元测试。
    //!
    //! 覆盖：
    //! - P0：port 范围 / mode 枚举 / NUL 字符拒绝
    //! - P0：llama_server_path 与 models_dir 必须存在
    //! - P3：默认值合理性
    use super::*;

    fn base_cfg() -> AppConfig {
        AppConfig {
            // 指向真实存在的目录，确保目录检查通过
            models_dir: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            ..AppConfig::default()
        }
    }

    // ---- P0: 端口 ----
    #[test]
    fn validate_rejects_port_zero() {
        let mut cfg = base_cfg();
        cfg.port = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_port_in_range() {
        let mut cfg = base_cfg();
        cfg.port = 10897;
        assert!(cfg.validate().is_ok());
    }

    // ---- P0: 模式 ----
    #[test]
    fn validate_rejects_unknown_mode() {
        let mut cfg = base_cfg();
        cfg.mode = "evil-mode".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_all_known_modes() {
        for mode in ["normal", "advanced", "pro"] {
            let mut cfg = base_cfg();
            cfg.mode = mode.to_string();
            assert!(cfg.validate().is_ok(), "模式 {} 应当通过", mode);
        }
    }

    // ---- P0: NUL 字符防护 ----
    #[test]
    fn validate_rejects_nul_in_custom_command() {
        let mut cfg = base_cfg();
        cfg.mode = "pro".to_string();
        cfg.custom_command = "llama-server\0 --evil".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_nul_in_extra_args() {
        let mut cfg = base_cfg();
        cfg.extra_args = "--prompt \"\0evil\"".to_string();
        assert!(cfg.validate().is_err());
    }

    // ---- P0: 路径必须存在 ----
    #[test]
    fn validate_rejects_nonexistent_models_dir() {
        let mut cfg = base_cfg();
        cfg.models_dir = "C:\\绝对\\不\\存在\\的\\目录".to_string();
        assert!(cfg.validate().is_err());
    }

    // ---- P0: 数值范围 ----
    #[test]
    fn validate_rejects_ctx_size_out_of_range() {
        let mut cfg = base_cfg();
        cfg.ctx_size = 0; // < 128
        assert!(cfg.validate().is_err());
        cfg.ctx_size = 1_000_000_000; // > 1_048_576
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_n_gpu_layers_out_of_range() {
        let mut cfg = base_cfg();
        cfg.n_gpu_layers = -100; // < -1
        assert!(cfg.validate().is_err());
        cfg.n_gpu_layers = 999; // > 200
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_mtp_draft_out_of_range() {
        let mut cfg = base_cfg();
        cfg.mtp_draft_n_max = 999;
        assert!(cfg.validate().is_err());
    }

    // ---- P3: schema 版本号 ----
    #[test]
    fn default_has_current_version() {
        let cfg = AppConfig::default();
        assert_eq!(cfg._v, CURRENT_CONFIG_VERSION);
    }
}
