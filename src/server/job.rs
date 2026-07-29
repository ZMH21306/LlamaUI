// Windows Job Object 绑定：保证父进程任何方式死亡时，子进程也会被内核回收。
//
// 背景：
//   - Cargo.toml `panic = "abort"` 意味着 panic 时 RAII（包括 ServerProcess::drop）不运行。
//   - 仅靠 Drop 守不住「主程序崩溃 → 子进程孤儿 → 占着 GPU 显存与端口」的场景。
//   - Windows 提供 JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE：父进程持有 Job Handle，
//     一旦 Handle 被关闭（无论主进程正常退出、panic、TaskKill、停电内核清理），
//     操作系统会立即终止绑定到该 Job 的所有进程。
//
// 设计：
//   - `Job` 仅 Windows 平台存在；非 Windows 平台是空类型（drop 时无操作）。
//   - `assign_to_job(child: &Child)` 必须在 spawn 之后、detach 之前调用。
//   - `Job` 实例由 ServerInner 持有；ServerInner drop 时会丢弃 Job handle，
//     触发子进程终止。

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JobObjectExtendedLimitInformation, JOBOBJECT_BASIC_LIMIT_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

/// Windows Job Object 包装。Drop 时关闭 handle，触发子进程终止。
#[cfg(windows)]
pub struct Job {
    handle: HANDLE,
}

#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    /// 创建带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object。
    /// 失败时返回底层错误（通常是权限不足）。
    pub fn create() -> io::Result<Self> {
        // 安全：传 null 给 CreateJobObjectW = 不指定 SECURITY_ATTRIBUTES + 名字
        // （不能被其它进程通过名字打开）。
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // 设置 limit flags：KILL_ON_JOB_CLOSE 是关键。
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..unsafe { std::mem::zeroed() }
        };
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            // 设置失败：关闭 handle，不留半成品。
            unsafe { CloseHandle(handle); }
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    /// 将已存在的进程 handle 绑定到本 Job。
    /// 必须在 spawn 之后立即调用，否则子进程存在一个不属于任何 Job 的窗口。
    ///
    /// `process_handle` 通常通过 `OpenProcess` 拿到（需要 PROCESS_SET_QUOTA +
    /// PROCESS_TERMINATE 权限）。我们直接复用 `Child` 内部的 raw handle
    /// （`as_raw_handle` 在 std 上是 unstable，但 tokio 的 Child 暴露了它）。
    #[cfg(windows)]
    pub fn assign_process(&self, process_handle: HANDLE) -> io::Result<()> {
        if process_handle.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process handle is null",
            ));
        }
        // 安全：调用方保证 handle 有效。
        let ok = unsafe { AssignProcessToJobObject(self.handle, process_handle) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// **推荐路径（P1-1 修复）**：通过 PID 调用 `OpenProcess` 获取**稳定**
    /// 进程 handle（不复用 `Child::raw_handle()` 的伪句柄），再绑到 Job。
    ///
    /// 为什么不用 `Child::raw_handle()`：
    ///   - `tokio::process::Child::raw_handle()` 在 Windows 上返回的是**伪句柄**
    ///     （`HANDLE(-1)`），依赖 `cmd.spawn()` 调用 `CreateProcess` 时**立即**
    ///     被 `AssignProcessToJobObject` 捕获。
    ///   - 伪句柄仅在**本进程内**有效，且 tokio 未来若改用 `DuplicateHandle` 后
    ///     再传 raw handle，会出现「Job 绑成功但立即失效」的诡异 bug。
    ///   - 显式 `OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, pid)` 拿到
    ///     真句柄，行为跨 tokio 版本稳定。
    ///
    /// 返回的 `HANDLE` 由调用方负责 `CloseHandle`（在 Job drop 后调用）。
    #[cfg(windows)]
    pub fn open_process_handle(pid: u32) -> io::Result<HANDLE> {
        // 安全：pid 来自 spawn() 后的 child.id()，调用方保证有效；权限位
        // PROCESS_SET_QUOTA 是 AssignProcessToJobObject 必需的。
        let h = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE,
                0, // bInheritHandle = FALSE
                pid,
            )
        };
        if h.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(h)
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // 关闭 handle → Windows 自动 kill 所有已 bind 的子进程。
        unsafe { CloseHandle(self.handle); }
    }
}

// ============ 非 Windows 平台的占位 ============
// 当前项目主要面向 Windows（llama-server.exe），但保留跨平台编译的可能。
// 在 Linux/macOS 上，panic=abort 也会跳过 Drop，所以严格的等价方案是
// prctl(PR_SET_PDEATHSIG) / kqueue NOTE_EXIT，但需要 unsafe + libc。
// 本次修复先在 Windows 上闭环；其它平台保持现有 Drop 行为（dev 模式 default = unwind）。
#[cfg(not(windows))]
pub struct Job;

#[cfg(not(windows))]
impl Job {
    pub fn create() -> std::io::Result<Self> {
        Ok(Job)
    }
    pub fn assign_process(&self, _process_handle: std::os::raw::c_void) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! P0 修复验证：Job Object 的「创建 + 进程绑定 + Drop 触发回收」
    //!
    //! 验证策略（Windows 平台）：
    //! 1) Job::create() 必须返回 Ok
    //! 2) 创建一个真子进程（cmd.exe /C exit 0）→ 拿 raw handle → bind 到 Job
    //! 3) **不调用** stop，直接 drop Job → 验证子进程被内核回收
    //!
    //! 注：第 3 步通过 cmd.exe 的 wait_with_output() 检测：
    //!   - 若 drop 生效 → 子进程提前被 kill，wait 返回非 0 / 异常退出
    //!   - 若 drop 未生效 → wait 正常返回 0
    //!
    //! 由于 cmd.exe 在 Win11 上几毫秒就跑完，测试中故意用 `timeout 5`（5 秒）
    //! 拉长窗口，确保有足够时间让 drop 介入。

    #[cfg(windows)]
    use std::os::windows::io::AsRawHandle;
    #[cfg(windows)]
    use std::process::Command;
    // 把父模块的 Job 引入到 tests 模块作用域，否则 #[cfg(windows)] 下的
    // `Job::create()` 会因「cannot find type `Job` in this scope」编译失败。
    use super::Job;

    #[cfg(windows)]
    #[test]
    fn job_create_succeeds() {
        // 仅验证 API 调用成功
        let job = Job::create();
        assert!(job.is_ok(), "Job::create() 必须在 Windows 上成功");
    }

    #[cfg(windows)]
    #[test]
    fn drop_job_kills_bound_child() {
        use std::time::Duration;

        // 1) 创建一个会跑 5 秒的子进程（cmd.exe /C timeout 5 > NUL）
        let mut child = Command::new("cmd.exe")
            .args(["/C", "timeout", "5", ">NUL"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn cmd.exe 失败");
        // std::process::Child::id() 在 stable 上返回 u32（非 Option），
        // 0 仅在「子进程尚未初始化完成」时短暂出现，spawn 后必有有效 PID。
        let pid = child.id();
        assert!(pid > 0, "子进程必须有有效 PID（>0），got {}", pid);

        // 2) 创建 Job 并绑子进程
        let job = Job::create().expect("Job::create 失败");
        let raw = child.as_raw_handle();
        job.assign_process(raw).expect("assign_process 失败");

        // 3) 不调 child.kill() / wait() —— 模拟 panic/abort 路径
        drop(job); // ← 触发 KILL_ON_JOB_CLOSE

        // 4) 给 OS 一点时间回收
        std::thread::sleep(Duration::from_millis(500));

        // 5) 现在再尝试 wait：如果 drop 生效，child 已被 kill，try_wait 返回 Some
        //    如果 drop 失效，try_wait 仍然返回 None（因为 timeout 5 还在跑）
        let still_running = child.try_wait().expect("try_wait 失败").is_none();
        assert!(
            !still_running,
            "drop Job 后子进程（PID {}）必须被回收（KILL_ON_JOB_CLOSE）",
            pid
        );
    }

    /// P1-1 验证：open_process_handle 用 PID 拿到稳定句柄
    #[cfg(windows)]
    #[test]
    fn open_process_handle_returns_non_null() {
        // 用当前进程 PID 4 字节（GetCurrentProcessId 一定有效）
        let pid = std::process::id();
        let h = Job::open_process_handle(pid).expect("open_process_handle 失败");
        assert!(!h.is_null(), "handle 必须非空");
        // 立即关闭（不依赖 Job.drop）
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(h);
        }
    }

    /// P1-1 验证：对不存在的 PID（u32::MAX）返回 Err 而非 panic
    #[cfg(windows)]
    #[test]
    fn open_process_handle_invalid_pid_returns_err() {
        let result = Job::open_process_handle(u32::MAX);
        // u32::MAX 不可能是有效 PID；OpenProcess 应返回 NULL → Err
        assert!(result.is_err(), "无效 PID 必须返回 Err 而不是 null handle");
    }

    #[cfg(not(windows))]
    #[test]
    fn job_stub_on_non_windows() {
        // 非 Windows 平台：Job 是占位类型
        let job = Job::create();
        assert!(job.is_ok());
    }
}
