# AI 执行契约 — LlamaUI

> 本文档是 AI（包括 Trae/GitHub Copilot 等）执行项目操作时的**唯一权威指令来源**。
> 自然语言指令按本表映射，不得自行推断执行路径。

---

## 指令映射表

| 用户自然语言 | AI 执行操作 |
|------------|------------|
| **提交代码** / **commit** / **保存改动** | 按「Commit 规范」生成消息并执行 `git add` + `git commit` |
| **发布新版本** / **release** / **打标签** | 运行 `.\scripts\release.ps1`（若工作区干净且有功能/修复变更） |
| **发布 v0.6.0** / **release v0.6.0** | 运行 `.\scripts\release.ps1 -Version 0.6.0` |
| **更新 CHANGELOG** / **更新日志** | 运行 `.\scripts\generate-changelog.ps1 -Version x.y.z` |
| **本地构建** / **build** / **编译** | 运行 `cargo build --offline` |
| **运行测试** / **test** | 运行 `cargo test` |
| **lint 检查** / **clippy** | 运行 `cargo clippy` |
| **开发模式** / **dev** | 运行 `cargo tauri dev` |

---

## Commit 规范（Conventional Commits）

```
<type>(<scope>): <subject>
```

### Type 与版本影响

| Type | 含义 | 自动版本递增 |
|------|------|-------------|
| `feat` | 新功能 | MINOR +1 |
| `fix` | Bug 修复 | PATCH +1 |
| `perf` | 性能优化 | PATCH +1 |
| `refactor` | 重构（不增功能、不修 bug） | 无 |
| `docs` | 文档变更 | 无 |
| `style` | 代码格式 | 无 |
| `test` | 测试相关 | 无 |
| `build` | 构建/依赖 | 无 |
| `ci` | CI/CD 配置 | 无 |
| `chore` | 杂项维护 | 无 |
| `revert` | 回滚 | — |

### Subject 规则
- 使用中文，祈使语气
- 不超过 50 个字符，句尾不加句号
- 描述做了什么（而非"改了什么"）

### Scope（常用）
`gpu`、`llama`、`update`、`logging`、`config`、`cmd`、`ui`、`core`、`build`、`deps`、`*`（无 scope）

### 示例

```
feat(gpu): 添加 AMD GPU ROCm 后端检测支持
fix(update): 修复 GitHub API 403 问题，补充 Accept header
perf(llama): 优化 llama-server 下载进度计算减少 CPU 占用
refactor(logging): 统一使用 tracing 替代 eprintln
chore(deps): 升级 tokio 至 1.40
```

---

## 发布前检查清单（必须全部通过，否则停止）

执行 `release` 前，AI 必须依次验证：

1. **工作区干净**
   ```powershell
   git status --porcelain
   ```
   输出为空才继续；否则先提示用户提交未完成的改动。

2. **编译通过**
   ```powershell
   cargo build --offline
   ```
   无错误才继续。

3. **测试通过**
   ```powershell
   cargo test
   ```
   全部 pass 才继续。

4. **无未提交 CHANGELOG 条目**
   检查 `CHANGELOG.md` 是否存在 `## [Unreleased]`，不存在则先运行 `generate-changelog.ps1` 生成条目。

5. **目标 tag 不存在**
   ```powershell
   git tag -l "v${Version}"
   ```
   若已有此 tag 则报错停止，不可覆盖远端 tag。

6. **默认分支可写**
   ```powershell
   git symbolic-ref refs/remotes/origin/HEAD
   ```
   确认结果为 `refs/remotes/origin/main`。

---

## 版本号管理（⚠️ 关键）

LlamaUI 有**两个版本源，必须同步修改**：

| 文件 | 字段 | 路径 |
|------|------|------|
| `Cargo.toml` | `version = "x.y.z"` | 根目录 |
| `tauri.conf.json` | `"version": "x.y.z"` | 根目录 |

**任何发布操作都必须同时更新这两个文件，缺一不可。**  
`release.ps1` 脚本已内置此逻辑，AI 无需手动编辑。

### 版本号规则（SemVer）

- **MAJOR**（x）：破坏性变更（配置 schema 不兼容、API 破坏性修改）
- **MINOR**（y）：新功能（向后兼容）
- **PATCH**（z）：Bug 修复（向后兼容）

### 自动递增逻辑

分析上次 tag 以来的 commit：
- 有 `BREAKING CHANGE` 或 `!` → MAJOR +1，MINOR=0，PATCH=0
- 有 `feat` → MINOR +1，PATCH=0
- 仅有 `fix`/`perf` → PATCH +1
- 无功能变更 → 版本号不变，提示用户

---

## 发布流程（完整步骤）

```
1. 检查工作区干净（git status）
2. 编译验证（cargo build --offline）
3. 测试验证（cargo test）
4. 运行 generate-changelog.ps1 更新 CHANGELOG
5. 运行 release.ps1 自动：
   a. 读取当前版本（从 Cargo.toml）
   b. 分析 commit 确定语义化版本（major/minor/patch）
   c. 同步更新 Cargo.toml + tauri.conf.json
   d. 提交版本更新到 main
   e. 打 v*x.y.z tag 并推送到远端
6. GitHub Actions (release.yml) 自动触发
   → 7 平台并行构建（cargo tauri build）
   → 收集产物（NSIS exe + MSI + zip，deb + AppImage + tar.gz，dmg + tar.gz）
   → 生成 SHA256SUMS.txt
   → 创建 GitHub Release（共 20 个文件）
```

---

## CI 工作流参考

- 触发条件：推送 `v*` tag 或手动 `workflow_dispatch`
- 构建矩阵：7 平台（Windows x64/x86/arm64、Linux x64/arm64、macOS x64/arm64）
- 发布地址：https://github.com/ZMH21306/LlamaUI/releases
- 产物命名：`LlamaUI_{VERSION}_{OS}_{ARCH}.{ext}`
- 产物总数：**20 个文件**（Windows 9 + Linux 6 + macOS 4 + SHA256SUMS.txt）

---

## 常见错误与处理

| 错误现象 | 原因 | 处理方式 |
|---------|------|---------|
| `release.ps1` 报"工作区有未提交变更" | 有未 commit 的文件 | 先 commit，再执行 release |
| tag 已存在报错 | 同一版本被发布过两次 | 删除本地 tag 后重试，或手动 bump 版本号 |
| `tauri.conf.json` 与 `Cargo.toml` 版本号不一致 | 手动修改了一个文件 | 以 `Cargo.toml` 为准，同步更新 `tauri.conf.json` |
| `git push origin main` 失败 | 分支名不是 main / 无权限 | 检查 `git remote show origin`，手动推送 |
| CHANGELOG 没有 `[Unreleased]` 段落 | 从未发布过或手动删除了 | `generate-changelog.ps1` 会 prepend，结果正常 |

---

## 本地开发命令速查

```powershell
# 编译（离线）
cargo build --offline

# 运行应用
cargo run

# 开发模式（热重载）
cargo tauri dev

# 测试
cargo test

# Lint
cargo clippy

# 清理
cargo clean           # 释放 ~15GB target/ 目录

# 发布（推荐用脚本）
.\scripts\release.ps1
.\scripts\release.ps1 -Version 0.6.0
.\scripts\generate-changelog.ps1 -Version 0.6.0

# Git
git add <具体文件>    # 禁止 git add .
git commit -m "type(scope): subject"
git push origin main
git tag -a v0.6.0 -m "Release v0.6.0"
git push origin v0.6.0
```

---

## 与 ClassIn-DL 的差异

| 维度 | ClassIn-DL | LlamaUI |
|------|-----------|---------|
| 版本源数量 | 1（`.csproj`） | 2（`Cargo.toml` + `tauri.conf.json`） |
| 发布前额外检查 | 无 | 编译 + 测试必须通过 |
| CI 产物数量 | ~7 个 | 20 个 |
| README 自动更新 | 有（update-readme.yml） | 无 |
