//! Tauri command 适配层。
//!
//! 把 `crate::server` / `crate::config` / `crate::detect` / `crate::init` 等领域
//! 模块的核心能力翻译成前端可调用的 IPC 端点。
//!
//! # 模块拆分（按职责）
//!
//! - [`server_cmd`] 服务进程控制：`start_server` / `stop_server` / `restart_server` /
//!   `get_status` / `get_logs` / `clear_logs`
//! - [`config_cmd`] 配置读写：`load_config` / `save_config`
//! - [`detect_cmd`] 自动检测：`detect_llama_server` / `detect_models_dir` /
//!   `cancel_detection` / `check_models_dir`
//! - [`init_cmd`] 启动初始化：`run_initialization`
//! - [`system_cmd`] 杂项：`open_external_url`（外部 URL 打开）
//!
//! 所有命令共享 [`AppState`] 单例（由 Tauri 注入）。
//!
//! # 设计原则
//!
//! - 适配层保持极薄：所有业务逻辑仍由 `crate::server` 等领域模块拥有；
//!   command 只做「参数解构 → 调领域 API → 错误转换」。
//! - 错误返回全部为 `Result<_, String>`，与 Tauri IPC 协议对齐。
//! - 单元测试集中在各子模块底部，便于按职责定位。
//!
//! # Clippy 例外
//!
//! Tauri 2.x 的 `#[tauri::command]` 宏要求参数按值传递（`State<'_, T>` 是
//! 内部引用，但签名上必须 by value），这与 clippy `needless_pass_by_value`
//! 误报。在 crate 级别豁免该 lint。

#![allow(clippy::needless_pass_by_value)]

use parking_lot::Mutex;

use crate::detect::CancelFlag;

pub mod config_io_cmd;
pub mod config_cmd;
pub mod detect_cmd;
pub mod download_cmd;
pub mod export_cmd;
pub mod gpu_cmd;
pub mod init_cmd;
pub mod model_cmd;
pub mod plugin_cmd;
pub mod recovery_cmd;
pub mod remote_cmd;
pub mod server_cmd;
pub mod system_cmd;
pub mod update_cmd;

/// Shared application state — managed by Tauri.
///
/// 字段说明：
/// - `server`：服务进程管理（`ServerProcess` 单例）
/// - `config`：配置存储（`ConfigStore` 单例）
/// - `detect_cancels`：当前正在进行的检测的取消标志列表
///   （parking_lot::Mutex，改自 std::sync::Mutex 以避免 panic 时的毒化问题；
///   详见 P0-1）。Vec 而非 Option 用来支持并发检测：检测启动时 push
///   一个新 flag，检测完成后 retain 移除自身；`cancel_detection` 会
///   遍历全部置 true 后清空。
/// - `model_manager`：多模型管理（目录索引 + 快速切换）
/// - `remote_server_manager`：远程服务器连接管理
/// - `plugin_manager`：插件系统管理器
pub struct AppState {
    /// 服务进程管理单例。
    pub server: std::sync::Arc<crate::server::ServerProcess>,
    /// 配置存储单例。
    pub config: std::sync::Arc<crate::config::ConfigStore>,
    /// 当前正在进行的检测的取消标志列表（支持并发检测场景）。
    pub detect_cancels: Mutex<Vec<crate::detect::CancelFlag>>,
    /// 多模型管理（目录索引 + 快速切换）。
    pub model_manager: std::sync::Arc<crate::model_management::ModelManager>,
    /// 远程服务器管理。
    pub remote_server_manager: std::sync::Arc<crate::remote_server::RemoteServerManager>,
    /// 插件系统管理器。
    pub plugin_manager: std::sync::Arc<crate::plugin_framework::PluginManager>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            server: std::sync::Arc::new(crate::server::ServerProcess::new()),
            config: std::sync::Arc::new(crate::config::ConfigStore::new()),
            detect_cancels: Mutex::new(Vec::new()),
            model_manager: std::sync::Arc::new(crate::model_management::ModelManager::new()),
            remote_server_manager: std::sync::Arc::new(
                crate::remote_server::RemoteServerManager::new(),
            ),
            plugin_manager: std::sync::Arc::new(crate::plugin_framework::PluginManager::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================
#[cfg(test)]
mod tests {
    //! 跨子模块共享的回归测试。子模块专属的测试放各子模块底部。

    use super::*;
    use std::sync::atomic::Ordering;

    /// P0-1 关键回归：模拟一个线程在持锁时 panic，验证后续 lock() 仍能返回 guard。
    ///
    /// 修复前：使用 `std::sync::Mutex`，panic 会让 Mutex 进入 poisoned 状态，
    ///         后续 `.lock().unwrap()` 会再次 panic → 整个 IPC 通道崩溃。
    /// 修复后：使用 `parking_lot::Mutex`，无 poison 概念，panic 后下一个 lock
    ///         调用直接拿到 guard。
    #[test]
    fn mutex_survives_panic_in_holder() {
        let mutex = std::sync::Arc::new(parking_lot::Mutex::new(0));

        // 子线程持锁后 panic
        let m2 = std::sync::Arc::clone(&mutex);
        let handle = std::thread::spawn(move || {
            let _guard = m2.lock();
            // 故意 panic
            panic!("模拟持锁时崩溃");
        });
        let _ = handle.join();

        // 修复后会通过：parking_lot::Mutex 没有 poison 概念，lock() 直接返回 guard
        let mut g = mutex.lock();
        *g = 42;
        drop(g);

        // 二次加锁仍可用
        let g2 = mutex.lock();
        assert_eq!(*g2, 42);
    }

    /// P1-13（DEFECT-019）回归：`detect_cancels` 使用 `Vec<CancelFlag>`，
    /// 并发检测场景下 `cancel_detection` 能同时取消所有进行中的检测，
    /// 且保留 `Arc::ptr_eq` 移除自身的能力。
    #[test]
    fn cancel_detection_handles_concurrent_detects() {
        let cancels: std::sync::Arc<Mutex<Vec<CancelFlag>>> =
            std::sync::Arc::new(Mutex::new(vec![]));
        let f1 = crate::detect::new_cancel_flag();
        let f2 = crate::detect::new_cancel_flag();
        cancels.lock().push(f1.clone());
        cancels.lock().push(f2.clone());

        // 模拟 cancel_detection 行为
        {
            let mut g = cancels.lock();
            for c in g.iter() {
                c.store(true, Ordering::Relaxed);
            }
            g.clear();
        }

        assert!(f1.load(Ordering::Relaxed), "f1 必须被取消");
        assert!(f2.load(Ordering::Relaxed), "f2 必须被取消");
        assert!(cancels.lock().is_empty());

        // 验证 ptr_eq 区分不同 Arc：f1 与 f2 指向不同分配
        assert!(!std::sync::Arc::ptr_eq(&f1, &f2));
        // 自身相等
        let f1_clone = f1.clone();
        assert!(std::sync::Arc::ptr_eq(&f1, &f1_clone));
    }
}
