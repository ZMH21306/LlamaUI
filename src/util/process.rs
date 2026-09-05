//! 静默子进程封装模块。
//!
//! 提供 `silent_command()` 和 `silent_tokio_command()` 函数，
//! 在 Windows 下自动设置 `CREATE_NO_WINDOW` 标志，
//! 确保子进程不会弹出控制台窗口。
//!
//! # 使用方式
//!
//! ```rust,ignore
//! use crate::util::process::silent_command;
//!
//! let mut cmd = silent_command("curl");
//! cmd.args(["-s", "https://example.com"]);
//! let output = cmd.output()?;
//! ```
//!
//! # Windows 行为
//!
//! 在 Windows 上，所有通过 `Command::new()` 启动的 GUI 子进程（如 curl.exe、7z.exe 等）
//! 默认会创建一个控制台窗口（即使它们的 stdin/stdout/stderr 都是 piped/null）。
//! 设置 `CREATE_NO_WINDOW` (0x08000000) 可以完全抑制该窗口。

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;
use tokio::process::Command as TokioCommand;

/// Windows CREATE_NO_WINDOW 标志，防止子进程创建控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 创建一个 `std::process::Command`，在 Windows 上自动设置 CREATE_NO_WINDOW 标志。
///
/// 在非 Windows 平台上，此函数等价于 `Command::new(program)`。
///
/// # 示例
///
/// ```rust,ignore
/// let mut cmd = silent_command("nvidia-smi");
/// cmd.args(["--query-gpu=name", "--format=csv,noheader"]);
/// let output = cmd.output()?;
/// ```
#[inline]
pub fn silent_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        // 需要导入 std::os::windows::process::CommandExt 才能调用 creation_flags
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 创建一个 `tokio::process::Command`，在 Windows 上自动设置 CREATE_NO_WINDOW 标志。
///
/// 在非 Windows 平台上，此函数等价于 `tokio::process::Command::new(program)`。
///
/// # 示例
///
/// ```rust,ignore
/// let mut cmd = silent_tokio_command("nvidia-smi");
/// cmd.args(["--query-gpu=name", "--format=csv,noheader"]);
/// let output = cmd.output().await?;
/// ```
#[inline]
pub fn silent_tokio_command(program: &str) -> TokioCommand {
    let mut cmd = TokioCommand::new(program);
    #[cfg(windows)]
    {
        // tokio::process::Command::creation_flags 同样接受 u32（Windows only）
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

