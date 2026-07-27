//! 启动初始化的步骤 ②：驱动与 llama 安装检查。
//!
//! 包含：
//! - `step_install_check`：入口函数
//! - `check_driver`：探测 GPU 驱动（带超时）
//! - `try_install_llama`：尝试通过系统包管理器自动补齐

use tauri::AppHandle;
use tokio::process::Command;

use crate::config::AppConfig;
use crate::log::emit_log_to;

use super::STEP_INSTALL;

/// ② 驱动与 llama 安装检查（缺失时尝试自动补齐）
pub(super) async fn step_install_check(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    emit_log_to(
        app,
        "system",
        "开始检查 llama-server 安装",
        Some(STEP_INSTALL),
    );

    // 自定义路径优先
    if let Some(custom) = &cfg.llama_server_path {
        if !custom.is_empty() {
            let p = std::path::Path::new(custom);
            if p.exists() {
                emit_log_to(
                    app,
                    "system",
                    &format!("自定义路径可用：{}", custom),
                    Some(STEP_INSTALL),
                );
                check_driver(app).await;
                emit_log_to(app, "system", "安装检查通过", Some(STEP_INSTALL));
                return Ok(());
            } else {
                let msg = format!("自定义路径不存在：{}", custom);
                emit_log_to(app, "system", &msg, Some(STEP_INSTALL));
                return Err(msg);
            }
        }
    }

    // PATH 检查
    if let Ok(p) = which::which("llama-server") {
        emit_log_to(
            app,
            "system",
            &format!("llama-server 已在 PATH 中：{}", p.display()),
            Some(STEP_INSTALL),
        );
        check_driver(app).await;
        emit_log_to(app, "system", "安装检查通过", Some(STEP_INSTALL));
        return Ok(());
    }

    emit_log_to(
        app,
        "system",
        "llama-server 不在 PATH 中，尝试自动补齐...",
        Some(STEP_INSTALL),
    );

    match try_install_llama(app).await {
        Ok(msg) => {
            emit_log_to(app, "system", &msg, Some(STEP_INSTALL));
            // 验证安装结果
            if which::which("llama-server").is_err() {
                let msg =
                    "自动安装完成，但 PATH 中仍找不到 llama-server，请重启终端后再试".to_string();
                emit_log_to(app, "system", &msg, Some(STEP_INSTALL));
                return Err(msg);
            }
        }
        Err(e) => {
            emit_log_to(
                app,
                "system",
                &format!("自动安装失败：{}", e),
                Some(STEP_INSTALL),
            );
            emit_log_to(app, "system", "请手动安装 llama.cpp：", Some(STEP_INSTALL));
            emit_log_to(
                app,
                "system",
                "  • Windows: winget install ggerganov.llama.cpp",
                Some(STEP_INSTALL),
            );
            emit_log_to(
                app,
                "system",
                "  • macOS:   brew install llama.cpp",
                Some(STEP_INSTALL),
            );
            emit_log_to(
                app,
                "system",
                "  • Linux:   参考 https://github.com/ggerganov/llama.cpp",
                Some(STEP_INSTALL),
            );
            return Err(e);
        }
    }

    check_driver(app).await;
    emit_log_to(app, "system", "安装检查通过", Some(STEP_INSTALL));
    Ok(())
}

/// 检测系统是否安装了 GPU 驱动（仅信息性检查，不阻断流程）
///
/// P2-3 修复：nvidia-smi 包裹 `tokio::time::timeout(3s)`。
/// 原因：某些驱动半死状态下 nvidia-smi 会无限挂起（已知 Windows NVIDIA
/// 驱动 issue），导致整个 ② 步骤卡死、初始化永远不返回。
/// 超时后记为"未检测到"并继续（不阻断后续流程）。
async fn check_driver(app: &AppHandle) {
    // 单次 nvidia-smi 探测的超时上限。3s 已经远高于正常情况（<200ms）。
    const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

    #[cfg(windows)]
    {
        // 通过 nvidia-smi 探测 NVIDIA 驱动
        match tokio::time::timeout(PROBE_TIMEOUT, Command::new("nvidia-smi").output()).await {
            Ok(Ok(out)) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let first = stdout.lines().next().unwrap_or("").trim();
                emit_log_to(
                    app,
                    "system",
                    &format!("检测到 NVIDIA 驱动：{}", first),
                    Some(STEP_INSTALL),
                );
            }
            Ok(Ok(_)) => {
                emit_log_to(
                    app,
                    "system",
                    "未检测到 NVIDIA 驱动（如使用纯 CPU 推理可忽略）",
                    Some(STEP_INSTALL),
                );
            }
            Ok(Err(e)) => {
                emit_log_to(
                    app,
                    "system",
                    &format!("nvidia-smi 启动失败：{}（如使用纯 CPU 推理可忽略）", e),
                    Some(STEP_INSTALL),
                );
            }
            Err(_) => {
                emit_log_to(
                    app,
                    "system",
                    &format!(
                        "nvidia-smi 探测超时（{}s），跳过（可能为驱动半死）",
                        PROBE_TIMEOUT.as_secs()
                    ),
                    Some(STEP_INSTALL),
                );
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let probe = tokio::time::timeout(PROBE_TIMEOUT, Command::new("nvidia-smi").output()).await;
        let has_nvidia = matches!(probe, Ok(Ok(o)) if o.status.success());
        if has_nvidia {
            emit_log_to(
                app,
                "system",
                "检测到 NVIDIA 驱动（nvidia-smi 可用）",
                Some(STEP_INSTALL),
            );
        } else if probe.is_err() {
            emit_log_to(
                app,
                "system",
                &format!("nvidia-smi 探测超时（{}s）", PROBE_TIMEOUT.as_secs()),
                Some(STEP_INSTALL),
            );
        } else if std::path::Path::new("/dev/dri").exists() {
            emit_log_to(
                app,
                "system",
                "检测到 /dev/dri（可能为 AMD/Intel GPU）",
                Some(STEP_INSTALL),
            );
        } else {
            emit_log_to(
                app,
                "system",
                "未检测到 GPU（将使用 CPU 推理）",
                Some(STEP_INSTALL),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        emit_log_to(
            app,
            "system",
            "macOS 将使用 Metal 加速（如可用）",
            Some(STEP_INSTALL),
        );
    }
}

/// 尝试调用系统包管理器安装 llama-server
async fn try_install_llama(app: &AppHandle) -> Result<String, String> {
    #[allow(unused_assignments)]
    let mut last_err = String::from("无可用包管理器");

    #[cfg(windows)]
    {
        // 1) winget
        emit_log_to(
            app,
            "system",
            "尝试使用 winget 安装 llama.cpp...",
            Some(STEP_INSTALL),
        );
        match Command::new("winget")
            .args([
                "install",
                "--id",
                "ggerganov.llama.cpp",
                "-e",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ])
            .output()
            .await
        {
            Ok(out) if out.status.success() => return Ok("通过 winget 安装 llama.cpp 成功".into()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                last_err = format!("winget 失败：{}", stderr.lines().next().unwrap_or(""));
                emit_log_to(
                    app,
                    "system",
                    &format!("winget 失败：{}", last_err),
                    Some(STEP_INSTALL),
                );
            }
            Err(e) => {
                last_err = format!("winget 不可用：{}", e);
                emit_log_to(app, "system", &last_err, Some(STEP_INSTALL));
            }
        }

        // 2) scoop
        emit_log_to(
            app,
            "system",
            "尝试使用 scoop 安装 llama.cpp...",
            Some(STEP_INSTALL),
        );
        match Command::new("scoop")
            .args(["install", "llama.cpp"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => return Ok("通过 scoop 安装 llama.cpp 成功".into()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                last_err = format!("scoop 失败：{}", stderr.lines().next().unwrap_or(""));
                emit_log_to(
                    app,
                    "system",
                    &format!("scoop 失败：{}", last_err),
                    Some(STEP_INSTALL),
                );
            }
            Err(e) => {
                last_err = format!("scoop 不可用：{}", e);
                emit_log_to(app, "system", &last_err, Some(STEP_INSTALL));
            }
        }

        // 3) choco
        emit_log_to(
            app,
            "system",
            "尝试使用 choco 安装 llama.cpp...",
            Some(STEP_INSTALL),
        );
        match Command::new("choco")
            .args(["install", "-y", "llama.cpp"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => return Ok("通过 choco 安装 llama.cpp 成功".into()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                last_err = format!("choco 失败：{}", stderr.lines().next().unwrap_or(""));
                emit_log_to(
                    app,
                    "system",
                    &format!("choco 失败：{}", last_err),
                    Some(STEP_INSTALL),
                );
            }
            Err(e) => {
                last_err = format!("choco 不可用：{}", e);
                emit_log_to(app, "system", &last_err, Some(STEP_INSTALL));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        emit_log_to(
            app,
            "system",
            "尝试使用 brew 安装 llama.cpp...",
            Some(STEP_INSTALL),
        );
        match Command::new("brew")
            .args(["install", "llama.cpp"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => return Ok("通过 brew 安装 llama.cpp 成功".into()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                last_err = format!("brew 失败：{}", stderr.lines().next().unwrap_or(""));
                emit_log_to(
                    app,
                    "system",
                    &format!("brew 失败：{}", last_err),
                    Some(STEP_INSTALL),
                );
            }
            Err(e) => {
                last_err = format!("brew 不可用：{}", e);
                emit_log_to(app, "system", &last_err, Some(STEP_INSTALL));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // apt
        emit_log_to(
            app,
            "system",
            "尝试使用 apt 安装 llama.cpp...",
            Some(STEP_INSTALL),
        );
        match Command::new("sh")
            .arg("-c")
            .arg("sudo -n apt-get install -y llama.cpp 2>&1 || apt-get install -y llama.cpp 2>&1")
            .output()
            .await
        {
            Ok(out) if out.status.success() => return Ok("通过 apt 安装 llama.cpp 成功".into()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                last_err = format!("apt 失败：{}", stderr.lines().next().unwrap_or(""));
                emit_log_to(
                    app,
                    "system",
                    &format!("apt 失败：{}", last_err),
                    Some(STEP_INSTALL),
                );
            }
            Err(e) => {
                last_err = format!("apt 不可用：{}", e);
                emit_log_to(app, "system", &last_err, Some(STEP_INSTALL));
            }
        }

        // dnf
        emit_log_to(
            app,
            "system",
            "尝试使用 dnf 安装 llama.cpp...",
            Some(STEP_INSTALL),
        );
        match Command::new("dnf")
            .args(["install", "-y", "llama.cpp"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => return Ok("通过 dnf 安装 llama.cpp 成功".into()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                last_err = format!("dnf 失败：{}", stderr.lines().next().unwrap_or(""));
                emit_log_to(
                    app,
                    "system",
                    &format!("dnf 失败：{}", last_err),
                    Some(STEP_INSTALL),
                );
            }
            Err(e) => {
                last_err = format!("dnf 不可用：{}", e);
                emit_log_to(app, "system", &last_err, Some(STEP_INSTALL));
            }
        }
    }

    Err(last_err)
}
