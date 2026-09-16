# 启动脚本
# 统一启动入口，参考 AI-IDE scripts/ 结构
#
# 用法:
#   .\scripts\start.ps1 [-Action start|dev|build|test|lint|release|audit]
#
# 分类规则:
#   feat  -> 新增 (Added)
#   fix   -> 修复 (Fixed)
#   perf  -> 性能 (Performance)
#   refactor -> 重构 (Changed)
#   BREAKING CHANGE / ! -> 破坏性变更 (Breaking Changes)

param(
    [string]$Action = "start"
)

# 修复 Windows 控制台中文乱码
$OutputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSDefaultParameterValues['*:Encoding'] = 'utf8'
chcp 65001 > $null

switch ($Action) {
    "start"  { cargo run }
    "dev"    { cargo tauri dev }
    "build"  { cargo build --release }
    "test"   { cargo test --lib }
    "lint"   { cargo clippy --all-targets --release }
    "release" { Write-Host "触发 CI 发布脚本，请选择 v* 标签" }
    "audit"  { cargo audit || Write-Host "⚠️ cargo audit 未安装或发现漏洞" }
    default { Write-Host "未知操作: $Action" }
}