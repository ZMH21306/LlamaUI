//! 自更新安装模块。
//!
//! 下载完成后：
//! 1. 解压 ZIP 到临时目录
//! 2. 校验 `llama-ui.exe` + `dist/`
//! 3. 备份当前 exe（`.old`）
//! 4. 替换可执行文件和前端资源
//! 5. 清理缓存文件

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::Result as AnyResult;
use sha2::Digest;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};
use zip::read::ZipArchive;

use crate::events::{UpdateDownloadProgress, UpdateState, EVT_UPDATE_DOWNLOAD_PROGRESS, EVT_UPDATE_STATE};

fn emit_progress(app: &AppHandle, progress: UpdateDownloadProgress) {
    let _ = app.emit(EVT_UPDATE_DOWNLOAD_PROGRESS, progress);
}

fn emit_state(app: &AppHandle, state: UpdateState) {
    let _ = app.emit(EVT_UPDATE_STATE, state);
}

fn extract_zip<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    target_dir: &Path,
) -> AnyResult<u64> {
    let mut total_bytes = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| anyhow::anyhow!("读取压缩条目失败: {}", e))?;

        let out_path = match entry.enclosed_name() {
            Some(p) => target_dir.join(p),
            None => {
                warn!(target: "UpdateInstall", name = %entry.name(), "跳过不安全的压缩条目");
                continue;
            }
        };

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = fs::File::create(&out_path)?;
        let mut buffer = Vec::new();
        entry.read_to_end(&mut buffer)?;
        file.write_all(&buffer)?;
        total_bytes += buffer.len() as u64;
    }
    Ok(total_bytes)
}

fn validate_extracted(root: &Path) -> AnyResult<()> {
    let exe_path = root.join("llama-ui.exe");
    if !exe_path.is_file() {
        return Err(anyhow::anyhow!("更新包中缺少 llama-ui.exe"));
    }

    let dist_dir = root.join("dist");
    for required in ["index.html", "main.js", "styles.css"] {
        let path = dist_dir.join(required);
        if !path.is_file() {
            return Err(anyhow::anyhow!("更新包 dist 目录缺少 {}", required));
        }
    }
    Ok(())
}

/// 递归复制目录。
fn copy_dir_all(src: &Path, dst: &Path) -> AnyResult<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 备份当前可执行文件并替换为更新版本，同时替换前端资源。
fn replace_exe_and_dist(exe_dir: &Path, extracted_root: &Path) -> AnyResult<()> {
    let current_exe = exe_dir.join("llama-ui.exe");
    let backup_exe = exe_dir.join("llama-ui.exe.old");

    if backup_exe.exists() {
        fs::remove_file(&backup_exe)?;
    }
    if current_exe.exists() {
        fs::rename(&current_exe, &backup_exe)?;
    }

    let new_exe = extracted_root.join("llama-ui.exe");
    fs::copy(&new_exe, &current_exe)?;

    let old_dist = exe_dir.join("dist");
    if old_dist.exists() {
        fs::remove_dir_all(&old_dist)?;
    }
    copy_dir_all(&extracted_root.join("dist"), &old_dist)?;
    Ok(())
}

/// 执行自更新安装。
///
/// 解压 ZIP → 校验完整性 → 备份旧文件 → 替换 exe 和 dist → 清理缓存。
/// 安装期间会通过 `EVT_UPDATE_DOWNLOAD_PROGRESS` 和 `EVT_UPDATE_STATE` 推送进度。
pub async fn install_update(
    app: &AppHandle,
    zip_path: &Path,
    total_bytes: u64,
) -> AnyResult<()> {
    info!(target: "UpdateInstall", zip = %zip_path.display(), "开始安装更新");

    emit_progress(app, UpdateDownloadProgress {
        stage: "extracting".to_string(),
        progress: 0.15,
        downloaded: 0,
        total: total_bytes,
        speed_mbps: 0.0,
        eta_secs: None,
        message: "正在解压更新包...".to_string(),
    });

    let temp_dir = std::env::temp_dir().join(format!("LlamaUI-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    let zip_file = fs::File::open(zip_path)
        .map_err(|e| anyhow::anyhow!("无法打开更新包: {}", e))?;
    let mut archive = ZipArchive::new(zip_file).map_err(|e| anyhow::anyhow!("更新包解析失败: {}", e))?;
    let extracted = extract_zip(&mut archive, &temp_dir)
        .map_err(|e| anyhow::anyhow!("解压失败: {}", e))?;

    emit_progress(app, UpdateDownloadProgress {
        stage: "verifying".to_string(),
        progress: 0.55,
        downloaded: extracted,
        total: total_bytes,
        speed_mbps: 0.0,
        eta_secs: None,
        message: "正在校验更新包...".to_string(),
    });

    validate_extracted(&temp_dir)?;

    let current_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("无法定位当前可执行文件: {}", e))?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无法解析当前可执行文件目录"))?;

    replace_exe_and_dist(exe_dir, &temp_dir)?;

    let _ = fs::remove_file(zip_path);
    let _ = fs::remove_dir_all(&temp_dir);

    emit_progress(app, UpdateDownloadProgress {
        stage: "installed".to_string(),
        progress: 1.0,
        downloaded: total_bytes,
        total: total_bytes,
        speed_mbps: 0.0,
        eta_secs: None,
        message: "更新安装完成，请重启应用程序".to_string(),
    });

    emit_state(app, UpdateState::Completed {
        new_version: String::new(),
    });

    info!(target: "UpdateInstall", "自更新安装完成");
    Ok(())
}

