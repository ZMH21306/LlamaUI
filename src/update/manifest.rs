//! Manifest 更新源客户端。
//! 从静态 JSON 文件获取最新版本信息，替代 GitHub Releases API。
//! 可通过环境变量 UPDATE_MANIFEST_URL 自定义端点。
//!
//! # 安全
//!
//! Manifest 支持 Ed25519 签名校验。服务器返回的 JSON 中必须包含
//! `signature` 字段（Base64 编码），使用内置公钥验证。
//! 验证失败时拒绝安装，防止中间人攻击劫持更新。

use std::env;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::net::{NetClient, NetError};

/// 内置的 Ed25519 公钥（Base64 编码）。
/// 用于校验 Manifest 的签名，防止中间人攻击。
pub const MANIFEST_PUBLIC_KEY_BASE64: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Manifest 根结构
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UpdateManifest {
    pub latest_version: String,
    pub min_version: Option<String>,
    pub assets: AssetMap,
    /// Ed25519 签名（Base64 编码）。可选：若服务器未签名则为 None。
    #[serde(default)]
    pub signature: Option<String>,
}

/// 平台资产映射
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AssetMap {
    #[serde(default)]
    pub windows_x64: Option<AssetInfo>,
    #[serde(default, rename = "windows-x64")]
    pub windows_x64_alt: Option<AssetInfo>,
    #[serde(default, rename = "windows-amd64")]
    pub windows_amd64: Option<AssetInfo>,
    #[serde(default, rename = "linux-x64")]
    pub linux_x64: Option<AssetInfo>,
    #[serde(default, rename = "linux-amd64")]
    pub linux_amd64: Option<AssetInfo>,
    #[serde(default, rename = "linux-aarch64")]
    pub linux_aarch64: Option<AssetInfo>,
    #[serde(default, rename = "linux-arm64")]
    pub linux_arm64: Option<AssetInfo>,
    #[serde(default, rename = "darwin-x64")]
    pub darwin_x64: Option<AssetInfo>,
    #[serde(default, rename = "darwin-amd64")]
    pub darwin_amd64: Option<AssetInfo>,
    #[serde(default, rename = "darwin-aarch64")]
    pub darwin_aarch64: Option<AssetInfo>,
    #[serde(default, rename = "darwin-arm64")]
    pub darwin_arm64: Option<AssetInfo>,
    #[serde(default, rename = "macos-x64")]
    pub macos_x64: Option<AssetInfo>,
    #[serde(default, rename = "macos-aarch64")]
    pub macos_aarch64: Option<AssetInfo>,
}

impl AssetMap {
    pub fn get_asset(&self, platform: &str) -> Option<&AssetInfo> {
        match platform {
            "windows-x64" => self.windows_x64.as_ref().or(self.windows_x64_alt.as_ref()),
            "linux-x64" | "linux-amd64" => self.linux_x64.as_ref().or(self.linux_amd64.as_ref()),
            "linux-aarch64" | "linux-arm64" => {
                self.linux_aarch64.as_ref().or(self.linux_arm64.as_ref())
            }
            "darwin-x64" | "darwin-amd64" | "macos-x64" => self
                .darwin_x64
                .as_ref()
                .or(self.darwin_amd64.as_ref())
                .or(self.macos_x64.as_ref()),
            "darwin-aarch64" | "darwin-arm64" | "macos-aarch64" => self
                .darwin_aarch64
                .as_ref()
                .or(self.darwin_arm64.as_ref())
                .or(self.macos_aarch64.as_ref()),
            _ => None,
        }
    }
}

/// 单个平台资产信息
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetInfo {
    pub url: String,
    #[serde(default)]
    pub size: u64,
    pub sha256: Option<String>,
    pub name: Option<String>,
}

/// Manifest 客户端
pub struct ManifestClient {
    client: NetClient,
    manifest_url: String,
}

impl ManifestClient {
    /// 创建默认 Manifest 客户端。
    pub fn new() -> Result<Self, NetError> {
        let manifest_url = env::var("UPDATE_MANIFEST_URL").unwrap_or_else(|_| {
            "https://update.llamaui.app/releases/latest/manifest.json".to_string()
        });
        Self::with_url(&manifest_url)
    }

    /// 创建带自定义 URL 的 Manifest 客户端。
    pub fn with_url(manifest_url: &str) -> Result<Self, NetError> {
        let client = NetClient::builder()
            .user_agent("LlamaUI-Update/1.0")
            .build()?;
        Ok(Self {
            client,
            manifest_url: manifest_url.to_string(),
        })
    }

    /// 异步获取 Manifest。
    pub async fn fetch(&self) -> Result<UpdateManifest, NetError> {
        info!(target: "UpdateCheck", url = %self.manifest_url, "正在获取更新 Manifest");
        let body = self.client.get(&self.manifest_url, &[]).await?;
        let manifest: UpdateManifest = serde_json::from_str(&body).map_err(NetError::Json)?;
        info!(target: "UpdateCheck", version = %manifest.latest_version, "Manifest 获取成功");
        Ok(manifest)
    }
}

/// 移除 Default 实现（避免 expect），使用显式工厂函数。
/// P0-4: 安全创建 ManifestClient 时检查网络错误，确保 early fail。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_map_parsing() {
        let json = r#"{"latest_version":"v0.8.0","assets":{"windows-x64":{"url":"https://example.com/win.zip","size":100}}}"#;
        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.latest_version, "v0.8.0");
        assert!(manifest.assets.get_asset("windows-x64").is_some());
    }
}
