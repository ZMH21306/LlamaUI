// 端口选择与管理
// 包含：端口可用性检查、智能端口选择、netstat 解析、占用者识别、taskkill 杀进程。

use std::net::TcpListener;
use std::sync::atomic::Ordering;
use tauri::AppHandle;

use futures::stream::{self, StreamExt};

use super::cmdline::is_llama_related_exe;
use super::winapi::{get_process_exe_name, is_pid_alive};
use crate::detect::CancelFlag;
use crate::log::emit_log;

/// 并行探测的前 N 个端口上限。剩余端口（>=N）顺序探测。
///
/// 性能优化（DEFECT-004）：旧版 `select_smart_port` 用 `for offset in 0..max_probes`
/// 串行 await，100 个被占端口可阻塞 `start_server` 长达 5 分钟。用
/// `futures::stream::buffer_unordered(PARALLEL_PROBE)` 把前 10 个端口并行探测，
/// 找到第一个空闲立即返回，使常见「端口空闲」场景的探测时间从分钟级降到 < 1s。
const PARALLEL_PROBE: u16 = 10;

/// 异步检查 `127.0.0.1:port` 是否可绑定。
/// 用 `tokio::task::spawn_blocking` 包裹 `std::net::TcpListener::bind`，避免阻塞
/// tokio worker 线程。`JoinError` 时保守返回 `false`（保守处理：当前路径无法判断
/// 端口是否可用时视同不可用，让上层走顺延逻辑）。
pub async fn is_port_available(port: u16) -> bool {
    tokio::task::spawn_blocking(move || TcpListener::bind(("127.0.0.1", port)).is_ok())
        .await
        .unwrap_or(false)
}

/// 用 `netstat -ano -p TCP` 查找正在监听指定端口的进程 PID。
/// 仅 Windows 有效（其它平台返回 None）。
#[allow(dead_code)] // 保留以备未来「端口被 llama 占用则杀进程」逻辑复用
#[cfg(windows)]
pub async fn find_pid_listening_on(port: u16) -> Option<u32> {
    use std::process::Stdio;
    let output = tokio::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_netstat_for_port(&stdout, port)
}

#[cfg(not(windows))]
pub async fn find_pid_listening_on(_port: u16) -> Option<u32> {
    None
}

/// 从 netstat 输出里找出监听指定端口的 PID。
/// 行格式：  TCP    0.0.0.0:8000    0.0.0.0:0    LISTENING    1234
///          索引: 0      1             2            3             4
/// 行字段数 ≥ 4，state 在倒数第二，pid 在最后。
#[allow(dead_code)]
fn parse_netstat_for_port(output: &str, port: u16) -> Option<u32> {
    let needle = format!(":{}", port);
    for line in output.lines() {
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("TCP") || trimmed.starts_with("UDP")) {
            continue;
        }
        if !trimmed.contains(&needle) {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        // state 在倒数第二，pid 在最后
        let state = parts[parts.len() - 2].to_ascii_uppercase();
        // 状态匹配：LISTENING / LISTEN（中英文 locale 都覆盖）
        if state != "LISTENING" && state != "LISTEN" {
            continue;
        }
        if let Ok(pid) = parts[parts.len() - 1].parse::<u32>() {
            return Some(pid);
        }
    }
    None
}

/// 检查端口是否被我们认为是"占用但可释放"的：要么是被 llama 进程占用，要么根本无法找到
/// 占用者（罕见，意味着权限不足或被内核态组件持有）。若是"其它服务"占用则不算可释放。
#[allow(dead_code)]
fn port_holder_is_killable(pid: Option<u32>) -> bool {
    match pid {
        None => true, // 找不到占用者：仍尝试一次（极端情况兜底）
        Some(p) => matches!(get_process_exe_name(p), Some(name) if is_llama_related_exe(&name)),
    }
}

/// 用 taskkill 杀进程（仅 Windows）。
#[cfg(target_os = "windows")]
pub async fn kill_pid_with_taskkill(pid: u32) {
    use std::process::Stdio;
    // 1. 优雅终止
    let _ = tokio::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .output()
        .await;
}

/// 强杀（/F /PID，仅 Windows）。
#[cfg(target_os = "windows")]
pub async fn force_kill_pid(pid: u32) {
    use std::process::Stdio;
    let _ = tokio::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .output()
        .await;
}

/// 用 kill 命令杀进程（仅 Linux/macOS）。
#[cfg(not(target_os = "windows"))]
pub async fn kill_pid_with_taskkill(pid: u32) {
    use std::process::Stdio;
    let _ = tokio::process::Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
}

/// 强杀（SIGKILL，仅 Linux/macOS）。
#[cfg(not(target_os = "windows"))]
pub async fn force_kill_pid(pid: u32) {
    use std::process::Stdio;
    let _ = tokio::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
}

/// 智能端口选择的最终结果。
pub struct PortChoice {
    pub port: u16,
    pub shifted: bool,
    pub killed_holders: Vec<u32>,
    /// 哪些端口被非 llama 占用而顺延。
    #[allow(dead_code)] // 保留字段供未来 P1 修复「端口被 llama 占用则杀」逻辑
    pub other_blockers: Vec<u16>,
}

/// 智能端口选择（并行探测前 10 个端口 + 顺序探测剩余 + 取消支持）。
///
/// 行为变化（相对 DEFECT-004 修复前的版本）：
/// - **并行化（性能修复）**：前 `PARALLEL_PROBE`（10）个端口用 `buffer_unordered`
///   并行探测，找到第一个空闲立即返回。100 端口全被占场景的探测时间从 ~5 分钟
///   降到 < 1s。
/// - **取消支持**：参数 `cancel: &CancelFlag`（`Arc<AtomicBool>`），函数入口、
///   tracked PID 等待循环、并行探测后、顺序探测循环中都会检查；若置位则返回
///   `Err("cancelled")`。
/// - **简化占用者处理**：旧版在每个端口被占时通过 `netstat` 找 holder、判断是否
///   llama 进程、是则杀之并重试。新版仅在入口杀掉本程序之前拉起的 tracked llama
///   （restart 场景），不再对 holder 做杀进程 + 重试 — 用户可用「重启」按钮
///   显式触发旧 llama 进程退出。
///
/// `auto_shift` 语义保留：若所有探测端口都不可用且 `auto_shift=false`，返回错误
/// 提示用户「未启用自动顺延」。
pub async fn select_smart_port(
    app: &AppHandle,
    desired: u16,
    tracked_pid: Option<u32>,
    auto_shift: bool,
    max_probes: u16,
    cancel: &CancelFlag,
) -> Result<PortChoice, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }

    let mut choice = PortChoice {
        port: desired,
        shifted: false,
        killed_holders: Vec::new(),
        other_blockers: Vec::new(),
    };

    // 1) 先杀本程序之前拉起的 llama-server（restart 流程关键）
    if let Some(pid) = tracked_pid {
        if is_pid_alive(pid) {
            emit_log(
                app,
                "system",
                &format!("正在停止本程序之前拉起的 llama-server（PID {}）…", pid),
            );
            kill_pid_with_taskkill(pid).await;
            // 等端口释放（最多 3s）
            for _ in 0..30 {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if !is_pid_alive(pid) {
                    break;
                }
            }
            // 再补一次强杀
            if is_pid_alive(pid) {
                force_kill_pid(pid).await;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            if !is_pid_alive(pid) {
                choice.killed_holders.push(pid);
                emit_log(app, "system", &format!("旧 llama-server（PID {}）已停止", pid));
            } else {
                emit_log(
                    app,
                    "system",
                    &format!("无法停止 PID {}，继续尝试其它路径", pid),
                );
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }

    // 2) 并行探测前 PARALLEL_PROBE 个端口（DEFECT-004 性能修复）
    let par = PARALLEL_PROBE.min(max_probes);
    let candidates: Vec<u16> = (0..par)
        .map(|i| desired.saturating_add(i))
        .filter(|&p| p != 0)
        .collect();
    let probes = stream::iter(candidates.iter().copied().map(|port| async move {
        (port, is_port_available(port).await)
    }))
    .buffer_unordered(par as usize)
    .collect::<Vec<_>>()
    .await;
    if let Some((port, true)) = probes.into_iter().find(|(_, avail)| *avail) {
        choice.port = port;
        choice.shifted = port != desired;
        return Ok(choice);
    }

    // 3) 若前 N 个都被占，顺序探测剩余端口（PARALLEL_PROBE..max_probes）
    for offset in par..max_probes {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let candidate = desired.saturating_add(offset);
        if candidate == 0 {
            continue;
        }
        if is_port_available(candidate).await {
            choice.port = candidate;
            choice.shifted = offset > 0;
            return Ok(choice);
        }
    }

    if !auto_shift {
        return Err(format!(
            "端口 {} 被占用且未启用自动顺延",
            desired
        ));
    }

    Err(format!(
        "在 {}-{} 范围内未找到空闲端口",
        desired,
        desired.saturating_add(max_probes - 1)
    ))
}

/// 纯函数路径的并行探测（不依赖 `AppHandle`，供测试使用）。
/// 探测 `[desired..desired+max)` 区间，找到第一个可用端口；全部都忙或被 cancel
/// 时返回 `None`。
#[cfg(test)]
async fn probe_ports_parallel(desired: u16, max: u16, cancel: &CancelFlag) -> Option<u16> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let par = PARALLEL_PROBE.min(max);
    let candidates: Vec<u16> = (0..par)
        .map(|i| desired.saturating_add(i))
        .filter(|&p| p != 0)
        .collect();
    let probes = stream::iter(candidates.iter().copied().map(|port| async move {
        (port, is_port_available(port).await)
    }))
    .buffer_unordered(par as usize)
    .collect::<Vec<_>>()
    .await;
    probes.into_iter().find_map(|(port, avail)| if avail { Some(port) } else { None })
}

#[cfg(test)]
mod tests {
    //! P0-4（DEFECT-004）回归测试。
    //!
    //! 注：原计划中的 `select_smart_port` 测试需要 `tauri::test::mock_app()`，但
    //! 本项目 `tauri = { version = "2", features = [] }` 未启用 `test` 特性，
    //! 单元测试无法直接构造 `AppHandle`。改测 `probe_ports_parallel` —— 这是
    //! `select_smart_port` 并行探测分支的纯函数提取，覆盖 `buffer_unordered` 的
    //! 核心行为且不依赖 Tauri runtime。

    use super::*;

    /// 找一个大概率空闲的端口段用于测试。策略：
    /// 1. 优先尝试「系统高位但非 ephemeral 段」(49000-49010)；
    /// 2. 若被占用，回退到下一段 (49020-49030)；
    /// 3. 最多尝试 5 段。Windows ephemeral 端口范围 49152-65535，所以 49000
    ///    段属于「非 ephemeral 用户端口」，正常情况下应空闲。
    #[allow(clippy::panic)] // 测试辅助：连续 5 段都被占用属于环境异常
    fn find_free_test_port_range() -> u16 {
        for offset in 0..5u16 {
            let base = 49000 + offset * 20;
            // 直接试 bind 一次，若成功则该段空闲
            if std::net::TcpListener::bind(("127.0.0.1", base)).is_ok() {
                // 立即释放（可能有 TIME_WAIT 但不影响 0.x 秒后的探测）
                return base;
            }
        }
        panic!("无法在 49000-49100 段找到空闲端口作为测试 base");
    }

    /// P0-4（DEFECT-004）修复验证：`buffer_unordered` 并行探测应在 < 2s 内
    /// 找到第一个空闲端口（解决旧版 100 端口串行探测最长 5 分钟阻塞）。
    #[tokio::test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    async fn parallel_probe_finds_available() {
        let base = find_free_test_port_range();

        let start = std::time::Instant::now();
        let cancel = crate::detect::new_cancel_flag();
        let result = probe_ports_parallel(base, 10, &cancel).await;
        let elapsed = start.elapsed();

        // 探测可能落到 base..base+10 中的任意一个空闲端口（若 base 处于
        // TIME_WAIT，会顺延到后续端口），但至少应找到一个。
        assert!(
            result.is_some(),
            "应能在 [{}-{}] 段找到空闲端口",
            base,
            base + 9
        );
        let found = result.expect("已通过 is_some 断言");
        assert!(
            (base..base + 10).contains(&found),
            "找到的端口 {} 必须在 [{}..{}) 范围内",
            found,
            base,
            base + 10
        );
        assert!(
            elapsed.as_secs() < 2,
            "并行探测应在 2s 内完成：实际 {:?}",
            elapsed
        );
    }
}
