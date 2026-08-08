# 构建脚本
# 用于统一构建流程，确保发布前构建通过

param(
    [string]$Target = "x86_64-pc-windows-msvc"
)

cargo build --release
cargo tauri build --target $Target
