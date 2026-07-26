//! `ServerProcess` 的 start / stop / Drop 兜底逻辑。
//!
//! 这是本 crate 最热路径（每分钟可能调用数十次的 metrics 循环与
//! 日志 pump 都从 `start` 派生的 task 中转），故注释与代码同样详细。
//!
//! 主要职责：
//! 1. `start`：派生 llama-server 子进程并启动 5 个后台任务
//!    （stdout / stderr reader + log pump + watcher + metrics sampler）。
//! 2. `stop`：优雅停机：abort 任务 → 关闭 Job handle → SIGTERM/wait → 强制 kill。
//! 3. `kill_orphan_on_drop`：主进程 drop 时的兜底清理。
//!
//! ## 关键修复点（行内注释有详述）
//! - P0-3 / DEFECT-003：5 个后台任务必须先在本地 Vec 中收集完毕，
//!   再在**单个 lock 块内**一次性注册到 `inner.tasks`。
//! - P0-6 / DEFECT-006：log 通道改用有界 mpsc（`log_channel::create`
//!   容量 2048），`try_send_or_count` 在满时丢弃并累加 `DROPPED_LOG_LINES`。
//! - P1-009：`kill_orphan_on_drop` 必须在同一 lock 块内清空
//!   `pid` / `started_at` / `active_port` / `status`。
//! - P1-1：改用 `Job::open_process_handle(pid)` 拿真句柄。
//! - P2-9：pro 模式 `custom_command` 解析用 let-else 替代 unwrap。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tauri::AppHandle;
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::config::AppConfig;
use crate::events::ServerStatus;
use crate::log::{emit_log, emit_status};

use super::cmdline::{expand_pro_vars, extract_port_from_argv, resolve_program, split_command_line, validate_pro_program};
use super::job::Job;
use super::log_channel::create as create_log_channel;
use super::port::select_smart_port;
use super::state::{ServerInner, ServerProcess};

/// How many ports to try when auto-port is enabled.
pub const MAX_PORT_PROBES: u16 = 100;
// 注：`METRICS_INTERVAL_MS` 已迁移到 [`super::metrics`] 模块（与其它
// metrics 常量集中管理），任务工厂从那里 import。

// ============================================================
// Drop 兜底（被 state.rs 的 Drop impl 调用）
// ============================================================

/// 主进程退出时清理子进程状态。
///
/// **这是兜底防线**：正常路径靠 Windows Job Object 在父进程
/// 死亡时自动回收子进程；Drop 仅在 Job Object 创建失败等异常场景触发。
///
/// P0-3 + P1-009 合并修复：必须在一个 lock 块内完成 tasks / child / job / pid /
/// started_at / active_port / status 全部清理。否则 partial task registration
/// 路径下即使 abort 了已注册的 task，`status` 仍为 Running 引发状态不一致。
pub(crate) fn kill_orphan_on_drop(inner: &Arc<parking_lot::Mutex<ServerInner>>) {
    let mut inner = inner.lock();
    // 1) abort 所有后台任务（pump / watcher / metrics）
    for h in std::mem::take(&mut inner.tasks) {
        h.abort();
    }
    // 2) 兜底：尝试直接 kill Child
    if let Some(mut c) = inner.child.take() {
        let _ = c.start_kill();
    }
    // 3) 关闭 Job handle（Windows 上会立即 kill 已绑定的子进程）。
    //    Drop 时触发，Linux/macOS 上 None 时 no-op。
    inner.job.take();
    // 4) P1-009 修复：清理 pid / started_at / active_port / status，
    //    避免 Drop 后 inner 残留「Running 状态但实际没进程」的不一致。
    inner.pid = None;
    inner.started_at = None;
    inner.active_port = None;
    inner.status = ServerStatus::Stopped;
}

// ============================================================
// start() —— 子进程派生 + 5 个后台任务
// ============================================================

impl ServerProcess {
    /// 启动 llama-server。若 `cfg.auto_port` 为 true 且 `cfg.port` 被占用，
    /// 将自动选择下一个空闲端口。
    /// 当 `cfg.mode == "pro"` 时，解析 `custom_command`（替换 `%%var%%` 变量后按空白拆分）
    /// 来启动；首项作为程序路径，其余作为参数。
    pub async fn start(&self, app: AppHandle, cfg: AppConfig) -> anyhow::Result<()> {
        // 1) 抢占 start_mutex，串行化所有 start/stop/restart 调用。
        //    防止两个并发 start() 都通过状态检查后同时 spawn，导致子进程孤儿泄漏。
        let _start_guard = self.start_mutex.lock().await;

        // 2) 已运行则拒绝再次启动
        {
            let inner = self.inner.lock();
            if inner.status == ServerStatus::Running || inner.status == ServerStatus::Starting {
                anyhow::bail!("服务已经在启动或运行中");
            }
        }

        // 3) 启动前做一次配置合法性校验（防止用户在保存后改坏了字段）
        cfg.validate()?;

        // ---- 解析最终生效的 program 和 args ----
        // pro 模式：从 custom_command 解析（替换变量、空白拆分），首项 = 程序；
        // custom_command 为空时回退到普通模式（自动检测 + 最简命令）。
        let (program, mut custom_argv): (String, Vec<String>) = if cfg.mode == "pro"
            && !cfg.custom_command.trim().is_empty()
        {
            let expanded = expand_pro_vars(&cfg.custom_command, &cfg);
            let tokens: Vec<String> = split_command_line(&expanded);
            // P2-9 修复：用 let-else 替代 tokens.is_empty() 检查 + iter.next().unwrap()。
            // - 原本两段式：先 is_empty() bail，再 iter.next().unwrap()，clippy 会标
            //   "unnecessary_unwrap"；现在 let-else 一次完成"判空+取首项"。
            // - 语义保持完全一致：tokens 为空时直接返回 Err。
            let mut iter = tokens.into_iter();
            let Some(prog) = iter.next() else {
                anyhow::bail!("专业模式命令为空");
            };
            // 安全校验：专业模式首 token 必须是 llama-server 相关可执行文件，
            // 阻止用户通过 `cmd /c calc` 等方式实现 RCE。
            let validated = validate_pro_program(&prog, &cfg)?;
            (validated, iter.collect())
        } else {
            (resolve_program(&cfg), Vec::new())
        };

        // ---- 模型目录校验（非 pro 模式校验自定义路径） ----
        if cfg.mode != "pro" {
            if cfg.models_dir.trim().is_empty() {
                anyhow::bail!("请先在左侧填写模型目录（包含 .gguf 文件的文件夹）");
            }
            let models_path = std::path::Path::new(&cfg.models_dir);
            if !models_path.exists() {
                anyhow::bail!("模型目录不存在：{}", cfg.models_dir);
            }
            if !models_path.is_dir() {
                anyhow::bail!("指定的路径不是目录：{}", cfg.models_dir);
            }
        }

        // ---- 选择端口（智能）----
        // 先确定 desired 端口，再调用 select_smart_port 走"先杀旧 llama、再尝试、占用者若是 llama 就杀掉、否则顺延"的逻辑。
        let desired_port = if cfg.mode == "pro" {
            extract_port_from_argv(&custom_argv).unwrap_or(cfg.port)
        } else {
            cfg.port
        };
        // 取得本程序上一次拉起的 llama-server PID
        let prev_pid = {
            let inner = self.inner.lock();
            inner.pid
        };
        // P0-4（DEFECT-004）：为本次 start 创建独立的取消 flag，select_smart_port
        // 内部会按需检查。flag 不与 detect 共享：detect 用 detect::new_cancel_flag()，
        // start 路径用本地新建的 flag，简单清晰。
        let start_cancel = crate::detect::new_cancel_flag();
        let choice = match select_smart_port(
            &app,
            desired_port,
            prev_pid,
            cfg.auto_port,
            MAX_PORT_PROBES,
            &start_cancel,
        )
        .await
        {
            Ok(c) => c,
            Err(msg) => {
                emit_log(&app, "system", &msg);
                anyhow::bail!(msg);
            }
        };
        let port = choice.port;
        if choice.shifted {
            emit_log(
                &app,
                "system",
                &format!("端口已自动从 {} 顺延到 {}", desired_port, port),
            );
        }
        // pro 模式：把新端口写回 custom_argv（如果有 --port 标记）
        if cfg.mode == "pro" && port != desired_port {
            if let Some((idx, _)) = custom_argv
                .iter()
                .enumerate()
                .find(|(_, a)| a.as_str() == "--port" || a.starts_with("--port="))
            {
                if custom_argv[idx].as_str() == "--port" {
                    if idx + 1 < custom_argv.len() {
                        custom_argv[idx + 1] = port.to_string();
                    }
                } else {
                    custom_argv[idx] = format!("--port={}", port);
                }
            }
        }

        // ---- 构建命令 ----
        let mut cmd = Command::new(&program);
        if cfg.mode == "pro" {
            // 专业模式：仅使用 custom_argv
            for token in &custom_argv {
                cmd.arg(token);
            }
        } else if cfg.mode == "normal" {
            // 普通模式：最简命令，最大兼容性。其它参数交给 llama-server 默认值。
            cmd.arg("--models-dir").arg(&cfg.models_dir);
            cmd.arg("--port").arg(port.to_string());
            // -ngl 99 让 llama-server 自行决定能卸多少层到 GPU；不支持时回退到 CPU。
            cmd.arg("-ngl").arg("99");
            // 显式绑定到 127.0.0.1（llama-server 默认值），方便用户知道访问地址。
            cmd.arg("--host").arg("127.0.0.1");
        } else {
            // 高级模式：路由模式（--models-dir）+ 完整参数
            cmd.arg("--models-dir").arg(&cfg.models_dir);
            cmd.arg("-c").arg(cfg.ctx_size.to_string());
            cmd.arg("--port").arg(port.to_string());

            // GPU 卸载层数：-1 = 全部，0 = 不使用，n = 指定层数
            if cfg.n_gpu_layers < 0 {
                cmd.arg("-ngl").arg("99");
            } else {
                cmd.arg("-ngl").arg(cfg.n_gpu_layers.to_string());
            }

            if cfg.flash_attn {
                cmd.arg("-fa");
            }

            if cfg.mtp {
                cmd.arg("--spec-draft-n-max")
                    .arg(cfg.mtp_draft_n_max.to_string());
            }

            // 额外自由参数：使用 shell-style 解析（保留引号内的空格）
            // 之前用 split_whitespace 会丢引号，导致 "--prompt "hi there"" 被拆成
            // ["--prompt", "hi", "there"]，引号完全消失。
            for token in split_command_line(&cfg.extra_args) {
                cmd.arg(token);
            }
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());
        // kill_on_drop：若 Child 在 stop 之前被 drop（异常路径），tokio 自动发送
        // SIGKILL/TerminateProcess。与 Windows Job Object 形成双重保险。
        cmd.kill_on_drop(true);

        // Windows 下隐藏控制台窗口
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // 更新状态：Starting
        {
            let mut inner = self.inner.lock();
            inner.status = ServerStatus::Starting;
            inner.active_port = Some(port);
        }
        emit_status(&app, ServerStatus::Starting);
        let start_msg = if cfg.mode == "pro" {
            format!(
                "正在启动 llama-server（专业模式） 端口 {}  可执行 {}  命令 {}",
                port,
                program,
                if cfg.custom_command.is_empty() {
                    "<无>".to_string()
                } else {
                    cfg.custom_command.clone()
                }
            )
        } else if cfg.mode == "normal" {
            format!(
                "正在启动 llama-server（普通模式·最简命令） 端口 {}  模型目录 {}  可执行 {}",
                port, cfg.models_dir, program
            )
        } else {
            format!(
                "正在启动 llama-server（高级模式） 端口 {}  模型目录 {}  可执行 {}",
                port, cfg.models_dir, program
            )
        };
        emit_log(&app, "system", &start_msg);

        // ---- Spawn ----
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                {
                    let mut inner = self.inner.lock();
                    inner.status = ServerStatus::Crashed;
                    inner.active_port = None;
                }
                emit_status(&app, ServerStatus::Crashed);
                emit_log(&app, "system", &format!("启动 llama-server 失败：{}", e));
                anyhow::bail!("启动 llama-server 失败：{}", e);
            }
        };

        let pid = child.id();

        // ---- 绑定子进程到 Windows Job Object ----
        // 父进程任何方式死亡（正常退出 / panic / TaskKill）时，内核自动 kill 子进程。
        // 失败时降级：仅记日志，不阻断启动（Drop 仍会兜底）。
        //
        // P1-1 修复：改用 `Job::open_process_handle(pid)` 拿**真句柄**，不再依赖
        // `child.raw_handle()`（伪句柄）的稳定性。
        let job = match Job::create() {
            Ok(j) => {
                #[cfg(windows)]
                {
                    // tokio::process::Child::id() 返回 Option<u32>：
                    // spawn 刚返回时 PID 一定存在（之前是 None）
                    let process_handle = pid.and_then(|p| {
                        match Job::open_process_handle(p) {
                            Ok(h) => Some(h),
                            Err(e) => {
                                emit_log(
                                    &app,
                                    "system",
                                    &format!("警告：OpenProcess 失败：{}（降级为 Drop 兜底）", e),
                                );
                                None
                            }
                        }
                    });
                    if let Some(h) = process_handle {
                        if let Err(e) = j.assign_process(h) {
                            emit_log(
                                &app,
                                "system",
                                &format!("警告：绑定子进程到 Job Object 失败：{}（降级为 Drop 兜底）", e),
                            );
                        }
                        // 立即关闭 process handle：Job 已持有引用，重复 handle 可关
                        unsafe {
                            windows_sys::Win32::Foundation::CloseHandle(h);
                        }
                    }
                }
                Some(j)
            }
            Err(e) => {
                emit_log(
                    &app,
                    "system",
                    &format!("警告：创建 Job Object 失败：{}（降级为 Drop 兜底）", e),
                );
                None
            }
        };

        // ---- Stream stdout/stderr through async readers ----
        // P0-3 (DEFECT-003) 修复：所有 5 个后台任务（stdout/stderr reader +
        // log pump + watcher + metrics sampler）必须先在本地 Vec 中收集完成，
        // 然后在单个 lock 块内一次性注册到 `inner.tasks`，避免 partial
        // registration 期间 Drop 只能 abort 部分 handle。
        //
        // P0-6 (DEFECT-006) 修复：改用有界 mpsc（log_channel::create 容量 2048）
        // 替代原本的 `mpsc::unbounded_channel`。原因：llama-server verbose 模式
        // 输出可达 10k 行/秒，无界通道在 WebView 卡顿时持续累积 → OOM。
        // reader 端用 `try_send_or_count`：容量满时主动丢弃并累加
        // DROPPED_LOG_LINES，避免 backpressure 阻塞 stdout/stderr 读取。
        //
        // 重构：5 个任务的具体实现已抽到 [`super::tasks`] 模块，
        // 本函数只剩"组装 + 单次原子注册"。
        let (tx, rx) = create_log_channel();
        let inner_arc = Arc::clone(&self.inner);
        let mut all_tasks: Vec<JoinHandle<()>> = Vec::with_capacity(5);
        if let Some(stdout) = child.stdout.take() {
            all_tasks.push(super::tasks::spawn_stdout_reader(stdout, tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            all_tasks.push(super::tasks::spawn_stderr_reader(stderr, tx.clone()));
        }
        all_tasks.push(super::tasks::spawn_log_pump(rx, app.clone(), Arc::clone(&inner_arc)));
        all_tasks.push(super::tasks::spawn_watcher(Arc::clone(&inner_arc), app.clone()));
        all_tasks.push(super::tasks::spawn_metrics_sampler(Arc::clone(&inner_arc), app.clone()));
        drop(tx); // 释放 sender 引用计数，让 pump 在 reader 退出后能正常结束

        // ---- 单次原子注册 ----
        // 5 个后台 task + child + job + pid + started_at + status = Running
        // 全部在同一个 lock 块内提交，确保 partial registration 期间
        // Drop 看到的要么是「完全 Running」、要么是「完全未注册」两种一致状态。
        {
            let mut inner = self.inner.lock();
            inner.child = Some(child);
            inner.pid = pid;
            inner.started_at = Some(Instant::now());
            inner.job = job;
            inner.tasks = all_tasks;
            // 注意：status = Running 必须在 tasks 注册完毕后才设置，
            // 否则 Drop 触发时 status 已 Running 但 tasks 为空会引发误判。
            inner.status = ServerStatus::Running;
        }
        emit_status(&app, ServerStatus::Running);

        Ok(())
    }

    /// Stop the running server gracefully. Sends SIGTERM (Unix) or kills (Windows),
    /// then waits up to 5 s before force-killing.
    pub async fn stop(&self, app: &AppHandle) -> anyhow::Result<()> {
        // 与 start() 互斥，避免与并发 start 相互覆盖 child / status
        let _start_guard = self.start_mutex.lock().await;
        // 先 abort 所有后台任务（pump / watcher / metrics），防止 stop 之后还
        // 持有 inner lock 或在旧 PID 上做无意义的 metric 计算。
        let tasks: Vec<JoinHandle<()>> = {
            let mut inner = self.inner.lock();
            std::mem::take(&mut inner.tasks)
        };
        for h in tasks {
            h.abort();
        }
        // 关闭 Job handle（Windows 上会立即 kill 已绑定的子进程；Job 已 drop 时 no-op）
        let _job = {
            let mut inner = self.inner.lock();
            inner.job.take()
        };
        drop(_job);

        let mut child = {
            let mut inner = self.inner.lock();
            match inner.child.take() {
                Some(c) => c,
                None => {
                    inner.status = ServerStatus::Stopped;
                    inner.pid = None;
                    inner.started_at = None;
                    inner.active_port = None;
                    drop(inner);
                    emit_status(app, ServerStatus::Stopped);
                    return Ok(());
                }
            }
        };

        emit_log(app, "system", "正在停止 llama-server（发送终止信号）...");

        #[cfg(unix)]
        {
            use std::process::Command as StdCommand;
            if let Some(pid) = child.id() {
                let _ = StdCommand::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child.start_kill();
        }

        let timeout = std::time::Duration::from_secs(5);
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ => {
                    if start.elapsed() > timeout {
                        let _ = child.kill().await;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }

        let _ = child.wait().await;

        {
            let mut inner = self.inner.lock();
            inner.status = ServerStatus::Stopped;
            inner.pid = None;
            inner.started_at = None;
            inner.active_port = None;
        }
        emit_status(app, ServerStatus::Stopped);
        emit_log(app, "system", "llama-server 已停止。");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// P1-009 合并测试：Drop 兜底必须完整清理 ServerInner 的全部运行时字段。
    /// 当前实现仅 abort tasks + 关 job + kill child，但 `pid` / `started_at` /
    /// `active_port` / `status` 全部保持 Running 状态——这正是 P1-009 要修的。
    #[tokio::test]
    async fn kill_orphan_on_drop_clears_all_runtime_state() {
        let sp = ServerProcess::new();
        // 模拟 start() 已部分注册：注入一个长跑 task 模拟 metrics 泄漏。
        // 用 `tokio::spawn` 需要运行时，#[tokio::test] 已提供。
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        {
            let mut inner = sp.inner.lock();
            inner.tasks.push(handle);
            inner.pid = Some(12345);
            inner.started_at = Some(Instant::now());
            inner.active_port = Some(19897);
            inner.status = ServerStatus::Running;
        }

        // Drop 兜底：必须清空全部运行时状态。
        kill_orphan_on_drop(&sp.inner);

        let inner = sp.inner.lock();
        assert!(
            inner.tasks.is_empty(),
            "tasks 应被 abort 并清空：实际 {} 个",
            inner.tasks.len()
        );
        assert!(inner.pid.is_none(), "pid 应被清空");
        assert!(inner.started_at.is_none(), "started_at 应被清空");
        assert!(inner.active_port.is_none(), "active_port 应被清空");
        assert_eq!(inner.status, ServerStatus::Stopped, "status 应回到 Stopped");
    }
}
