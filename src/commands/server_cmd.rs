//! 服务进程控制命令。
//!
//! 把 [`crate::server::ServerProcess`] 的核心能力暴露给前端：
//! - 生命周期：start / stop / restart
//! - 状态查询：get_status / get_logs / clear_logs
//!
//! 所有命令都是 `async`（即便底层只读），以便未来扩展时无需改 IPC 签名。

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::server::{LogLine, ServerStatus};

use super::AppState;

/// 服务状态响应（前端用来决定按钮可用性 / WebView URL）。
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// 当前服务状态机值。
    pub status: ServerStatus,
    /// 用户配置的端口（不变）。
    pub port: u16,
    /// 实际绑定的端口（auto_port 顺延后可能与 `port` 不同）。
    pub active_port: Option<u16>,
}

/// 启动 llama-server 子进程。
///
/// 行为：
/// - 串行化互斥：与 `stop_server` / `restart_server` 通过 `start_mutex` 互斥，
///   防止并发调用导致子进程孤儿泄漏。
/// - 端口：依据 `cfg.port` 与 `cfg.auto_port` 调用 `select_smart_port`：
///   占用时若占用者是 llama 进程则先 kill 再重试同一端口；否则顺延到下一端口
///   （最大 `MAX_PORT_PROBES` 次）。
/// - 模式：根据 `cfg.mode` 拼接不同命令：
///   - `normal`：最简命令（`--models-dir --port -ngl 99 --host 127.0.0.1`）
///   - `advanced`：路由模式 + 完整参数（`extra_args` 按 shell-style 拆分）
///   - `pro`：从 `cfg.custom_command` 解析（首 token 必须通过白名单校验）
///
/// 副作用：派生 stdout/stderr reader / log pump / watcher / metrics sampler
/// 4 个 tokio 任务，全部记入 `ServerInner.tasks`，stop 时统一 abort。
/// Windows 上额外创建 Job Object 绑定子进程，父进程任何方式死亡时内核会
/// 回收子进程。
#[tauri::command]
pub async fn start_server(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let cfg = state.config.get();
    state
        .server
        .start(app, cfg)
        .await
        .map_err(|e| e.to_string())
}

/// 停止 llama-server 子进程。
///
/// 流程：
/// 1. 抢占 start_mutex（与 start / restart 互斥）
/// 2. abort 所有派生任务（pump / watcher / metrics）
/// 3. drop Job handle（Windows 上立即 kill 已绑定的子进程）
/// 4. 发送 SIGTERM（Unix）或 `start_kill`（Windows）→ 等待 5s → 强杀
///
/// 若当前未运行则返回 `Ok(())`（幂等）。
#[tauri::command]
pub async fn stop_server(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.server.stop(&app).await.map_err(|e| e.to_string())
}

/// 重启 llama-server。
///
/// 等价于「先 `stop_server`，等 500ms 让端口释放，再 `start_server`」。
/// 第二次启动若失败会向上抛错（前半 stop 失败被忽略，因为若服务未运行
/// 时调用 stop 是合法的）。
#[tauri::command]
pub async fn restart_server(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Stop first (ignore error if not running)
    let _ = state.server.stop(&app).await;
    // Small delay so the port is released
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let cfg = state.config.get();
    state
        .server
        .start(app, cfg)
        .await
        .map_err(|e| e.to_string())
}

/// 获取当前服务状态、配置端口、实际绑定端口。
///
/// `active_port` 与 `port` 的差异：若启动时 `auto_port` 顺延到了别的端口，
/// `active_port` 是实际绑定的，`port` 是用户配置的。前端用它来决定是否更新
/// WebView URL。
#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> StatusResponse {
    let status = state.server.status();
    let port = state.config.get().port;
    let active_port = state.server.active_port();
    StatusResponse {
        status,
        port,
        active_port,
    }
}

/// 获取内存中累积的日志快照（最多 `MAX_LOG_LINES` 行）。
///
/// 前端启动时一次性拉取，重启后的历史日志通过此命令恢复。
/// 返回的 `LogLine` 按时间升序排列。
#[tauri::command]
pub fn get_logs(state: State<'_, AppState>) -> Vec<LogLine> {
    state.server.logs_snapshot()
}

/// 清空内存中累积的日志（不影响后端运行中的进程）。
///
/// 用户的「清空日志」按钮直接调用此命令。
#[tauri::command]
pub fn clear_logs(state: State<'_, AppState>) {
    state.server.clear_logs();
}

#[cfg(test)]
mod tests {
    //! 单元测试覆盖：状态响应字段顺序、序列化格式。
    use super::*;
    use serde_json;

    /// 验证 `StatusResponse` 序列化后包含前端依赖的字段名。
    /// 前端 `dist/main.js` 通过 `resp.status` / `resp.port` / `resp.active_port` 读取。
    #[test]
    fn status_response_field_names() {
        let r = StatusResponse {
            status: ServerStatus::Running,
            port: 10897,
            active_port: Some(10898),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"status\":\"Running\""), "status 字段：{}", s);
        assert!(s.contains("\"port\":10897"), "port 字段：{}", s);
        assert!(
            s.contains("\"active_port\":10898"),
            "active_port 字段：{}",
            s
        );
    }

    /// 验证 `ServerStatus` 序列化值与前端 JS 期望一致。
    /// 前端 dist/main.js 通过字符串字面量比对（"Running" / "Stopped" / ...）。
    #[test]
    fn server_status_serializes_to_expected_strings() {
        assert_eq!(
            serde_json::to_value(ServerStatus::Stopped).unwrap(),
            serde_json::json!("Stopped")
        );
        assert_eq!(
            serde_json::to_value(ServerStatus::Starting).unwrap(),
            serde_json::json!("Starting")
        );
        assert_eq!(
            serde_json::to_value(ServerStatus::Running).unwrap(),
            serde_json::json!("Running")
        );
        assert_eq!(
            serde_json::to_value(ServerStatus::Crashed).unwrap(),
            serde_json::json!("Crashed")
        );
    }
}
