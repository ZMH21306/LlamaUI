# 静默启动脚本 - 隐藏终端窗口
# 用法: cscript //nologo run.vbs

$OutputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
chcp 65001 > $null

Write-Output "Starting LlamaUI in hidden mode..."

try {
    cargo tauri dev --no-daemon
} catch {
    Write-Output "Error: $_"
}