fn main() {
    // Windows 目标需要链接 ntdll.lib 以使用 WDK API（NtQueryInformationProcess）
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rustc-link-lib=ntdll");
    }
    tauri_build::build()
}
