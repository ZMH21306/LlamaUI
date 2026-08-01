//! 配置导入/导出模块。
//!
//! 将 AppConfig 序列化为 JSON 字符串（导出）或从 JSON 字符串反序列化（导入），
//! 供前端通过系统文件对话框实现导入导出功能。

use crate::config::AppConfig;
use serde_json::Value;

/// 将配置导出为格式化的 JSON 字符串
pub fn export_config(cfg: &AppConfig) -> anyhow::Result<String> {
    let json = serde_json::to_string_pretty(cfg)?;
    Ok(json)
}

/// 从 JSON 字符串导入配置。
///
/// 校验规则：
/// - 必须是合法 JSON 且为对象
/// - 必须能完整反序列化为 AppConfig
/// - 版本号必须合理（> 0 且 < 1_000_000）
pub fn import_config(json_str: &str) -> anyhow::Result<AppConfig> {
    let value: Value = serde_json::from_str(json_str)?;
    if !value.is_object() {
        return Err(anyhow::anyhow!("JSON 必须是一个对象"));
    }
    let cfg: AppConfig = serde_json::from_value(value)
        .map_err(|e| anyhow::anyhow!("配置格式无效：{}", e))?;
    if cfg._v == 0 || cfg._v > 1_000_000 {
        return Err(anyhow::anyhow!("配置版本号 {} 不合理", cfg._v));
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn export_produces_valid_json() {
        let cfg = AppConfig::default();
        let json = export_config(&cfg).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_object());
    }

    #[test]
    fn roundtrip_default_config() {
        let original = AppConfig::default();
        let json = export_config(&original).unwrap();
        let restored = import_config(&json).unwrap();
        assert_eq!(original._v, restored._v);
    }

    #[test]
    fn import_rejects_invalid_json() {
        assert!(import_config("not json").is_err());
    }

    #[test]
    fn import_rejects_array() {
        assert!(import_config("[]").is_err());
    }

    #[test]
    fn import_rejects_string() {
        assert!(import_config("\"hello\"").is_err());
    }

    #[test]
    fn import_rejects_empty_object() {
        assert!(import_config("{}").is_err());
    }
}
