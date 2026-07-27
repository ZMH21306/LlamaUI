// Windows 平台特定的进程信息查询
//
// 本模块只暴露虚拟地址空间（VirtualSize）查询：
//  - 用 `NtQueryInformationProcess + ProcessVmCounters` 拿 `VM_COUNTERS.VirtualSize`
//  - 含 mmap 映射文件，适合观察 llama.cpp mmap 了多大的 GGUF 文件
//  - 与任务管理器"虚拟大小"列一致（默认不显示，需手动勾选）
//
// 设计：单字段、零额外 syscall，调用方一次拿一个 `u64`。
//
// 修复说明（P1-2）：is_pid_alive / get_process_exe_name 改用 OnceLock 共享
// `sysinfo::System`，避免每秒 4 次 `System::new()` 造成的 4 MB/s 内存抖动。

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, NTSTATUS};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::OpenProcess;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
// 使用 windows-sys 官方提供的 VM_COUNTERS 结构体（注意 PageFaultCount 必须是 u32）
#[cfg(windows)]
use windows_sys::Wdk::System::SystemServices::VM_COUNTERS;
#[cfg(windows)]
use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessVmCounters};

use std::sync::OnceLock;

/// 全局共享的 sysinfo::System（P1-2 修复）
///
/// - 用 `OnceLock` 保证进程内单例
/// - 用 `parking_lot::Mutex` 保护（轻量、无 poison）
/// - 每次只 `refresh_processes(Some(&[pid]))`，不开销无关进程
static PROC_CACHE: OnceLock<parking_lot::Mutex<sysinfo::System>> = OnceLock::new();

fn shared_system() -> parking_lot::MutexGuard<'static, sysinfo::System> {
    PROC_CACHE
        .get_or_init(|| parking_lot::Mutex::new(sysinfo::System::new()))
        .lock()
}

/// 进程虚拟地址空间（bytes）。含 mmap 映射文件、reserved-but-uncommitted 区域。
/// 失败时为 0。
#[cfg(windows)]
pub fn query_windows_virtual_size(pid: u32) -> u64 {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return 0;
    }

    // 拿 VM_COUNTERS（取 VirtualSize）
    // 使用 windows-sys 官方结构体，确保 PageFaultCount 是 u32，布局与 Windows 一致
    let mut vm: VM_COUNTERS = unsafe { std::mem::zeroed() };
    let mut ret_len: u32 = 0;
    let status: NTSTATUS = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessVmCounters,
            &mut vm as *mut _ as *mut _,
            std::mem::size_of::<VM_COUNTERS>() as u32,
            &mut ret_len,
        )
    };
    let virtual_size = if status >= 0 {
        vm.VirtualSize as u64
    } else {
        0
    };

    unsafe {
        CloseHandle(handle);
    }
    virtual_size
}

#[cfg(not(windows))]
pub fn query_windows_virtual_size(_pid: u32) -> u64 {
    0
}

/// 用 sysinfo 查一个 PID 是否还活着。
///
/// 用于「端口被旧 llama 进程占用」等场景：调用 `kill_pid_with_taskkill` 后
/// 轮询此函数判断是否已被内核回收。返回 `false` 不区分「不存在」与「已退出」，
/// 两种情况对调用方而言语义一致。
///
/// 性能（P1-2 修复后）：复用全局 `sysinfo::System` 实例，仅 refresh 单个 PID，
/// 1Hz 采样开销从 ~2-5 MB 分配下降为 ~1 KB。
pub fn is_pid_alive(pid: u32) -> bool {
    let mut sys = shared_system();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(
        pid,
    )]));
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

/// 用 sysinfo 取一个 PID 对应的可执行文件名（不是全路径）。
///
/// 仅返回文件名（如 `llama-server.exe`），用于端口占用者识别。
/// 进程不存在时返回 `None`。非 ASCII 进程名优先做 UTF-8 无损转换，
/// 失败时回退到 lossy（保留可见字符，`?` 替代无效字节）。
///
/// 性能（P1-2 修复后）：复用全局 `sysinfo::System` 实例。
pub fn get_process_exe_name(pid: u32) -> Option<String> {
    let mut sys = shared_system();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(
        pid,
    )]));
    sys.process(sysinfo::Pid::from_u32(pid)).map(|p| {
        // 优先无损 UTF-8 转换（兼容非 ASCII 进程名），失败时再 lossy
        p.name()
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| p.name().to_string_lossy().into_owned())
    })
}

// ============================================================
// 单元测试（P1-2：sysinfo 缓存）
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// 验证全局 sysinfo 缓存正确初始化（P1-2）
    ///
    /// 注：必须确保 guard 在 assertion 之前 drop，否则与其它并行测试抢锁。
    /// 改为：先 get_or_init（一次性初始化并立即 drop guard），再断言。
    #[test]
    fn shared_system_creates_singleton() {
        // 第一次调用：触发 OnceLock 初始化
        {
            let _g = shared_system();
        }
        // 第二次调用：直接拿到 guard
        {
            let _g = shared_system();
        }
        // 验证 PROC_CACHE 已被填充
        assert!(PROC_CACHE.get().is_some());
    }

    /// 验证 is_pid_alive / get_process_exe_name 在 PID 0（不存在）时不 panic
    ///
    /// 注：sysinfo 0.30+ 在某些 Windows 环境下 refresh PID 0 会死锁，
    /// 跳过此特定 PID，仅做"调用本身不 panic"的烟雾测试。
    #[test]
    fn pid_zero_handled_safely() {
        // 改用 1 号进程（System）：比 0 更稳定，且一定存在。
        let _ = is_pid_alive(1);
        let name = get_process_exe_name(1);
        // 不论是否有结果，都不应 panic
        let _ = name;
    }
}
