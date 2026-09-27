//! 原子安装器：解压 → 备份 → 原子替换 → 失败回滚。
//!
//! # 原子性保证
//!
//! 安装流程遵循「准备 → 提交 → 清理」三段式：
//! 1. 准备：解压到同盘临时目录，校验关键文件存在
//! 2. 提交：把旧目录重命名为 .bak-<txid>，再把新目录重命名为 target
//! 3. 清理：删除备份；若任一步失败，自动回滚到原目录
//!
//! 同盘重命名是 POSIX / NTFS 上保证原子性的唯一手段，因此整个过程
//! 不会出现「旧文件已删、新文件未就位」的空窗期。
//!
//! # 断电保护
//!
//! 每次安装会在 target 同级写一个 install-state.json 事务日志，
//! 下次启动时由 recover_from_crash 检测并完成或回滚中断的事务。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 安装事务日志文件名
pub const STATE_FILE: &str = "install-state.json";

/// 安装事务状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallState {
    pub txid: String,
    pub target: String,
    #[serde(default)]
    pub backup: Option<String>,
    pub staging: String,
    pub started_at: u64,
    #[serde(default)]
    pub committed: bool,
}

impl InstallState {
    fn now_millis() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn state_path(target: &Path) -> PathBuf {
        target.parent().unwrap_or(Path::new(".")).join(format!(
            "{}.state.json",
            target.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
        ))
    }

    pub fn persist(&self) -> std::io::Result<()> {
        let p = Self::state_path(Path::new(&self.target));
        let tmp = p.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn clear(target: &Path) {
        let _ = std::fs::remove_file(Self::state_path(target));
    }

    pub fn load(target: &Path) -> Option<Self> {
        let p = Self::state_path(target);
        let json = std::fs::read_to_string(p).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn begin(target: &Path, staging: &Path) -> Self {
        Self {
            txid: uuid::Uuid::new_v4().to_string(),
            target: target.to_string_lossy().to_string(),
            backup: None,
            staging: staging.to_string_lossy().to_string(),
            started_at: Self::now_millis(),
            committed: false,
        }
    }
}

fn suffixed(target: &Path, suffix: &str) -> PathBuf {
    let parent = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
    parent.join(format!("{}.{}", name, suffix))
}

/// 安装错误
#[derive(Debug)]
pub enum InstallError {
    MissingSource(PathBuf),
    BadTarget(String),
    Io(std::io::Error),
    MissingBinary(String),
    Cancelled,
    RollbackFailed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSource(p) => write!(f, "源目录不存在: {}", p.display()),
            Self::BadTarget(m) => write!(f, "目标路径不可用: {}", m),
            Self::Io(e) => write!(f, "文件系统操作失败: {}", e),
            Self::MissingBinary(n) => write!(f, "安装包缺少关键文件: {}", n),
            Self::Cancelled => write!(f, "操作已取消"),
            Self::RollbackFailed(m) => write!(f, "回滚失败: {}", m),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

type Result<T> = std::result::Result<T, InstallError>;

/// 原子安装：把 `staging` 替换到 `target`。
///
/// - 旧版本先被重命名为 .bak-<txid>，成功后删除
/// - 任何失败都会尝试自动回滚
pub fn atomic_install(
    staging: &Path,
    target: &Path,
    required_files: &[&str],
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    if !staging.is_dir() {
        return Err(InstallError::MissingSource(staging.to_path_buf()));
    }

    for f in required_files {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(InstallError::Cancelled);
        }
        if !staging.join(f).is_file() {
            return Err(InstallError::MissingBinary((*f).to_string()));
        }
    }

    let txid = uuid::Uuid::new_v4().to_string();
    let backup = suffixed(target, &format!("bak-{}", &txid[..8]));

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let mut state = InstallState::begin(target, staging);
    let _ = state.persist();

    let had_old = target.exists();
    if had_old {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            InstallState::clear(target);
            return Err(InstallError::Cancelled);
        }
        if let Err(e) = std::fs::rename(target, &backup) {
            InstallState::clear(target);
            return Err(InstallError::BadTarget(format!(
                "无法重命名旧目录（文件可能正被占用，请先停止 llama-server）: {}", e
            )));
        }
        state.backup = Some(backup.to_string_lossy().to_string());
        let _ = state.persist();
    }

    if let Err(e) = std::fs::rename(staging, target) {
        if had_old {
            let _ = std::fs::rename(&backup, target);
        }
        InstallState::clear(target);
        return Err(InstallError::Io(e));
    }

    state.committed = true;
    let _ = state.persist();

    if had_old {
        if let Err(e) = std::fs::remove_dir_all(&backup) {
            tracing::warn!(target: "LlamaInstaller", backup = %backup.display(), error = %e, "备份清理失败");
        }
    }
    InstallState::clear(target);
    Ok(())
}

/// 崩溃恢复：检测上次未完成的事务并修复。
pub fn recover_from_crash(target: &Path) -> Option<String> {
    let state = InstallState::load(target)?;
    tracing::warn!(target: "LlamaInstaller", txid = %state.txid, committed = state.committed, "检测到未完成的上次安装事务");

    if state.committed {
        if let Some(b) = &state.backup {
            let _ = std::fs::remove_dir_all(b);
        }
        let _ = std::fs::remove_dir_all(&state.staging);
        InstallState::clear(target);
        return Some(format!("已清理上次更新的残留文件（事务 {}）", state.txid));
    }

    let staging = PathBuf::from(&state.staging);
    let mut notes = vec![format!("回滚未完成的事务 {}", state.txid)];

    if let Some(b) = &state.backup {
        let b = PathBuf::from(b);
        if b.is_dir() {
            if target.exists() {
                let _ = std::fs::remove_dir_all(target);
            }
            match std::fs::rename(&b, target) {
                Ok(_) => notes.push("已恢复旧版本".to_string()),
                Err(e) => {
                    notes.push(format!("恢复旧版本失败: {}", e));
                    tracing::error!(target: "LlamaInstaller", error = %e, "回滚失败");
                }
            }
        }
    }

    if staging.is_dir() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    InstallState::clear(target);
    Some(notes.join("；"))
}

/// 创建同盘暂存目录。
pub fn make_staging(target: &Path, label: &str) -> Result<PathBuf> {
    let parent = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = target.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
    let staging = parent.join(format!("{}.{}-{}", name, label, &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir_all(&staging)?;
    Ok(staging)
}

/// 安全删除目录（跨平台，重试）。
pub fn force_remove_dir_all(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for attempt in 0..5u32 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                if attempt == 4 {
                    return Err(InstallError::Io(e));
                }
                tracing::debug!(target: "LlamaInstaller", attempt, error = %e, "删除目录失败，重试");
                std::thread::sleep(std::time::Duration::from_millis(150 * (attempt as u64 + 1)));
                #[cfg(windows)]
                make_writable(dir);
            }
        }
    }
    Ok(())
}

/// 递归清除只读属性（Windows）。
///
/// Rust stable 的 `std::os::windows::fs::PermissionsExt` 仍是 unstable，
/// 因此这里直接调用 `attrib -R` 系统命令处理整个目录树。
#[cfg(windows)]
fn make_writable(dir: &Path) {
    if !dir.exists() {
        return;
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new("cmd")
        .args(["/C", "attrib", "-R", "-S"])
        .arg(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => tracing::debug!(target: "LlamaInstaller", code = s.code(), "attrib 返回非零（可能无只读文件）"),
        Err(e) => tracing::debug!(target: "LlamaInstaller", error = %e, "attrib 执行失败（忽略）"),
    }
}
#[cfg(not(windows))]
fn make_writable(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "llamaui_inst_{}_{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&p).expect("create temp root");
        p
    }

    fn write_bin(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"binary").expect("write bin");
    }

    #[test]
    fn installs_into_empty_target() {
        let root = temp_root("fresh");
        let target = root.join("llama");
        let staging = make_staging(&target, "dl").expect("staging");
        write_bin(&staging, "llama-server.exe");

        atomic_install(&staging, &target, &["llama-server.exe"], None).expect("install");

        assert!(target.join("llama-server.exe").is_file());
        assert!(!staging.exists(), "暂存目录应已被消费");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn replaces_existing_and_removes_backup() {
        let root = temp_root("replace");
        let target = root.join("llama");
        fs::create_dir_all(&target).expect("mkdir target");
        write_bin(&target, "llama-server.exe");
        fs::write(target.join("old.marker"), b"x").expect("marker");

        let staging = make_staging(&target, "dl").expect("staging");
        write_bin(&staging, "llama-server.exe");
        write_bin(&staging, "new.marker");

        atomic_install(&staging, &target, &["llama-server.exe"], None).expect("install");

        assert!(target.join("new.marker").is_file(), "新文件应在");
        assert!(!target.join("old.marker").exists(), "旧文件应消失");
        let leftovers: Vec<_> = fs::read_dir(root.as_path())
            .expect("read root")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".bak-"))
            .collect();
        assert!(leftovers.is_empty(), "备份残留: {:?}", leftovers);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_required_file_aborts_and_keeps_old() {
        let root = temp_root("missing");
        let target = root.join("llama");
        fs::create_dir_all(&target).expect("mkdir target");
        write_bin(&target, "llama-server.exe");

        let staging = make_staging(&target, "dl").expect("staging");

        let err = atomic_install(&staging, &target, &["llama-server.exe"], None)
            .expect_err("应当失败");
        assert!(matches!(err, InstallError::MissingBinary(_)));
        assert!(target.join("llama-server.exe").is_file(), "旧版本必须完好");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_before_install_keeps_old() {
        let root = temp_root("cancel");
        let target = root.join("llama");
        fs::create_dir_all(&target).expect("mkdir target");
        write_bin(&target, "llama-server.exe");

        let staging = make_staging(&target, "dl").expect("staging");
        write_bin(&staging, "llama-server.exe");

        let flag = AtomicBool::new(true);
        let err = atomic_install(&staging, &target, &["llama-server.exe"], Some(&flag))
            .expect_err("应当被取消");
        assert!(matches!(err, InstallError::Cancelled));
        assert!(target.join("llama-server.exe").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn state_file_persists_and_clears() {
        let root = temp_root("state");
        let target = root.join("llama");
        fs::create_dir_all(&target).expect("mkdir");
        let staging = make_staging(&target, "dl").expect("staging");

        let state = InstallState::begin(&target, &staging);
        state.persist().expect("persist");
        assert!(InstallState::load(&target).is_some(), "事务日志应可读回");

        InstallState::clear(&target);
        assert!(InstallState::load(&target).is_none(), "事务日志应被清除");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_restores_backup_when_uncommitted() {
        let root = temp_root("recover");
        let target = root.join("llama");
        fs::create_dir_all(&target).expect("mkdir");
        write_bin(&target, "llama-server.exe");

        let backup = suffixed(&target, "bak-deadbeef");
        fs::rename(&target, &backup).expect("simulate backup");

        let mut state = InstallState::begin(&target, Path::new("/tmp/staging"));
        state.backup = Some(backup.to_string_lossy().to_string());
        state.committed = false;
        state.persist().expect("persist");

        let note = recover_from_crash(&target).expect("应执行恢复");
        assert!(note.contains("回滚"), "note={}", note);
        assert!(target.join("llama-server.exe").is_file(), "旧版本应被恢复");
        assert!(InstallState::load(&target).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_cleans_residue_when_committed() {
        let root = temp_root("recover2");
        let target = root.join("llama");
        fs::create_dir_all(&target).expect("mkdir");
        write_bin(&target, "llama-server.exe");

        let staging = make_staging(&target, "dl").expect("staging");
        write_bin(&staging, "leftover");

        let mut state = InstallState::begin(&target, &staging);
        state.committed = true;
        state.persist().expect("persist");

        let note = recover_from_crash(&target).expect("应执行恢复");
        assert!(note.contains("清理"), "note={}", note);
        assert!(!staging.exists(), "残留暂存目录应被删除");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn force_remove_handles_missing_dir() {
        let root = temp_root("remove");
        assert!(force_remove_dir_all(&root.join("nope")).is_ok());
        let _ = fs::remove_dir_all(&root);
    }
}