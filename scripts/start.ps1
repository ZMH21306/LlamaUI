# 启动脚本
# 统一启动流程，参考 AI-IDE scripts/ 结构

param(
    [string]$Action = "start"
)

switch ($Action) {
    "start" { cargo run }
    "dev" { cargo tauri dev }
    "build" { .\build\build.ps1 }
    "test" { cargo test }
    "lint" { cargo clippy }
    "release" { Write-Host "触发 CI 发布流程：推送 v* 标签" }
    default { Write-Host "未知操作: $Action" }
}
