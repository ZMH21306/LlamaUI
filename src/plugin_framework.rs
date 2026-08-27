//! 插件系统框架（v0.6 引入）。
//!
//! 提供可扩展的插件加载机制，允许第三方开发者扩展 LlamaUI 功能。
//!
//! # 设计目标
//!
//! - **热加载**：插件可在运行时加载/卸载，无需重启应用
//! - **沙箱隔离**：每个插件在独立命名空间中运行
//! - **回调机制**：插件可注册到生命周期钩子（启动、配置变更、事件）
//!
//! # 插件接口
//!
//! 所有插件必须实现 [`Plugin`] trait。示例插件结构：
//! ```rust,ignore
//! struct MyPlugin;
//!
//! impl Plugin for MyPlugin {
//!     fn name(&self) -> &str { "my-plugin" }
//!     fn on_init(&self, ctx: &PluginContext) { ... }
//!     fn on_config_changed(&self, _new_cfg: &AppConfig) { ... }
//! }
//! ```
//!
//! # 安全说明
//!
//! 当前版本（v0.6）插件框架为**实验性**，尚未实现完整沙箱隔离。
//! 仅支持本地文件系统加载的静态插件（`.so` / `.dll`），不支持远程下载执行。

use std::fmt;
use std::sync::Arc;
use parking_lot::Mutex;

use serde::Serialize;

use crate::config::AppConfig;

/// 插件上下文，由框架提供，插件可用于获取应用状态。
pub struct PluginContext {
    pub app_version: String,
    pub config: Arc<Mutex<AppConfig>>,
}

impl fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginContext")
            .field("app_version", &self.app_version)
            .finish()
    }
}

/// 插件生命周期钩子接口。
///
/// 所有插件必须实现此 trait。框架会在对应生命周期阶段调用这些方法。
pub trait Plugin: Send + Sync + std::fmt::Debug {
    /// 插件唯一名称（用于标识和冲突检测）。
    fn name(&self) -> &str;

    /// 插件版本（语义化版本字符串）。
    fn version(&self) -> &str {
        "0.0.0"
    }

    /// 插件描述（显示给用户）。
    fn description(&self) -> &str {
        ""
    }

    /// 插件初始化钩子（应用启动后调用一次）。
    fn on_init(&self, _ctx: &PluginContext) {}

    /// 配置变更钩子（每次配置保存后调用）。
    fn on_config_changed(&self, _new_cfg: &AppConfig) {}

    /// 插件卸载前的清理钩子。
    fn on_shutdown(&self, _ctx: &PluginContext) {}

    /// 返回插件支持的 Tauri 命令名称列表（用于注册）。
    fn commands(&self) -> Vec<&str> {
        Vec::new()
    }
}

/// 插件元数据（用于 UI 展示和冲突检测）。
#[derive(Debug, Clone, Serialize)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    /// 插件来源路径（本地文件路径）。
    pub source_path: Option<String>,
    /// 是否已启用。
    pub enabled: bool,
}

impl PluginMetadata {
    pub fn from_plugin<P: Plugin>(plugin: &P, source_path: Option<&str>) -> Self {
        Self {
            name: plugin.name().to_string(),
            version: plugin.version().to_string(),
            description: plugin.description().to_string(),
            source_path: source_path.map(|s| s.to_string()),
            enabled: true,
        }
    }
}

/// 插件管理器（注册/加载/卸载插件）。
#[derive(Debug)]
pub struct PluginManager {
    plugins: Arc<Mutex<Vec<Box<dyn Plugin>>>>,
    metadata: Arc<Mutex<Vec<PluginMetadata>>>,
}

impl PluginManager {
    /// 创建空的插件管理器。
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(Mutex::new(Vec::new())),
            metadata: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 注册一个插件。
    ///
    /// # 冲突检测
    /// 若已存在同名插件，返回 `Err` 而不覆盖。
    pub fn register<P: Plugin + 'static>(&self, plugin: P, source: Option<&str>) -> Result<(), String> {
        let name = plugin.name().to_string();
        let meta = PluginMetadata::from_plugin(&plugin, source);
        let mut plugins = self.plugins.lock();
        if plugins.iter().any(|p| p.name() == name) {
            return Err(format!("插件 '{}' 已注册，拒绝重复加载", name));
        }
        plugins.push(Box::new(plugin));
        drop(plugins);

        self.metadata.lock().push(meta);
        Ok(())
    }

    /// 卸载指定名称的插件。
    pub fn unregister(&self, name: &str) -> Result<(), String> {
        let mut plugins = self.plugins.lock();
        let pos = plugins
            .iter()
            .position(|p| p.name() == name)
            .ok_or_else(|| format!("插件 '{}' 不存在", name))?;
        plugins.remove(pos);
        drop(plugins);

        let mut metas = self.metadata.lock();
        metas.retain(|m| m.name != name);
        Ok(())
    }

    /// 获取所有已注册的插件元数据。
    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        self.metadata.lock().clone()
    }

    /// 初始化所有插件（应用启动时调用）。
    pub fn init_all(&self, ctx: &PluginContext) {
        let plugins = self.plugins.lock();
        for plugin in plugins.iter() {
            plugin.on_init(ctx);
        }
    }

    /// 配置变更时通知所有插件。
    pub fn notify_config_changed(&self, cfg: &AppConfig) {
        let plugins = self.plugins.lock();
        for plugin in plugins.iter() {
            plugin.on_config_changed(cfg);
        }
    }

    /// 关闭时清理所有插件。
    pub fn shutdown_all(&self, ctx: &PluginContext) {
        let plugins = self.plugins.lock();
        for plugin in plugins.iter() {
            plugin.on_shutdown(ctx);
        }
    }

    /// 获取支持的所有命令名称（去重）。
    pub fn all_commands(&self) -> Vec<String> {
        let plugins = self.plugins.lock();
        let mut cmds: Vec<String> = plugins
            .iter()
            .flat_map(|p| p.commands().into_iter().map(|s| s.to_string()))
            .collect();
        cmds.sort();
        cmds.dedup();
        cmds
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 示例插件：用于测试和演示插件框架。
#[allow(dead_code)]
#[derive(Debug)]
pub struct HelloPlugin {
    pub invoke_count: std::sync::Arc<parking_lot::Mutex<u32>>,
}

#[allow(dead_code)]
impl HelloPlugin {
    pub fn new() -> Self {
        Self {
            invoke_count: std::sync::Arc::new(parking_lot::Mutex::new(0)),
        }
    }
}

#[allow(dead_code)]
impl Plugin for HelloPlugin {
    fn name(&self) -> &str {
        "hello-plugin"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn description(&self) -> &str {
        "LlamaUI 示例插件：显示欢迎信息并统计调用次数"
    }

    fn on_init(&self, ctx: &PluginContext) {
        tracing::info!(
            target: "Plugin:hello",
            app_version = %ctx.app_version,
            "示例插件初始化完成"
        );
        *self.invoke_count.lock() += 1;
    }

    fn on_config_changed(&self, _cfg: &AppConfig) {
        *self.invoke_count.lock() += 1;
    }

    fn commands(&self) -> Vec<&str> {
        vec!["hello_plugin::get_invocation_count"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_registers_and_lists() {
        let mgr = PluginManager::new();
        // 使用空实现进行测试
        #[derive(Debug)]
        struct DummyPlugin;
        impl Plugin for DummyPlugin {
            fn name(&self) -> &str { "dummy" }
            fn version(&self) -> &str { "1.0.0" }
            fn description(&self) -> &str { "测试用" }
        }
        assert!(mgr.register(DummyPlugin, None).is_ok());
        let list = mgr.list_plugins();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "dummy");
    }

    #[test]
    fn manager_rejects_duplicate() {
        let mgr = PluginManager::new();
        #[derive(Debug)]
        struct D1;
        impl Plugin for D1 {
            fn name(&self) -> &str { "dup" }
        }
        #[derive(Debug)]
        struct D2;
        impl Plugin for D2 {
            fn name(&self) -> &str { "dup" }
        }
        assert!(mgr.register(D1, None).is_ok());
        assert!(mgr.register(D2, None).is_err());
    }

    #[test]
    fn manager_unregisters() {
        let mgr = PluginManager::new();
        #[derive(Debug)]
        struct U1;
        impl Plugin for U1 {
            fn name(&self) -> &str { "to-remove" }
        }
        mgr.register(U1, None).unwrap();
        assert!(mgr.unregister("to-remove").is_ok());
        assert!(mgr.list_plugins().is_empty());
    }
}
