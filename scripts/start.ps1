# 启动脚本
# 统一启动流程，参考 AI-IDE scripts/ 结构

param(
    [string]$Action = "start"
)

switch ($Action) {
    "start"  { cargo run }
    "dev"    { cargo tauri dev }
    "build"  { cargo build --release }
    "test"   { cargo test --lib }
    "lint"   { cargo clippy --all-targets --release }
    "release" { Write-Host "触发 CI 发布流程：推送 v* 标签" }
    "audit"  { cargo audit || Write-Host "⚠️ cargo audit 未安装或发现漏洞" }
    default { Write-Host "未知操作: $Action" }
}
