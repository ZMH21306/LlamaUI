// llama-server 进程管理模块（聚合层）
//
// 负责子进程的派生、监控、日志流转发以及优雅停机。
// 与前端通信通过：
//   * Tauri 命令（定义在 commands.rs）进行控制
//   * Tauri 事件："server-log"（逐行日志）、"server-status"（状态变化）、
//     "server-metrics"（周期性进程指标）
//
// 子模块拆分（按职责切分，单文件 ≤ 500 行）：
//   - state.rs         ServerProcess / ServerInner 状态 + 轻量访问器
//   - lifecycle.rs     start / stop / kill_orphan_on_drop（编排层）
//   - tasks.rs         5 个后台任务工厂（stdout/stderr reader + pump + watcher + metrics）
//   - log_truncate.rs  单行日志字节上限 + 截断函数
//   - log_channel.rs   有界 mpsc（2048 容量）日志通道
//   - cmdline.rs       命令行解析、路径处理、白名单校验
//   - port.rs          端口选择、netstat 解析、taskkill
//   - winapi.rs        Windows NTAPI 封装的内存查询
//   - job.rs           Windows Job Object 绑定子进程
//   - metrics.rs       Metrics 结构 + nvidia-smi 采样（带缓存）

pub mod cmdline;
pub mod job;
pub mod lifecycle;
pub mod log_channel;
pub mod log_truncate;
pub mod metrics;
pub mod port;
pub mod state;
pub mod tasks;
pub mod winapi;

// ============================================================
// 公共 re-export：仅保留外部代码实际依赖的类型
// ============================================================

// 进程管理主类型：commands / init 通过 `crate::server::ServerProcess` 访问
pub use state::ServerProcess;

// 公开类型 re-export：保持 `crate::server::ServerStatus` / `LogLine` 等
// 历史路径可用。真正的类型定义在 `crate::events`。
pub use crate::events::{LogLine, ServerStatus};
