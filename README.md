# LlamaUI

> LLM Desktop Console for managing llama-server

[![Release](https://img.shields.io/github/v/release/LlamaUI/LlamaUI?label=version&sort=semver)](https://github.com/LlamaUI/LlamaUI/releases)
[![License](https://img.shields.io/github/license/LlamaUI/LlamaUI)](LICENSE)
[![Tests](https://img.shields.io/github/actions/workflow/status/LlamaUI/LlamaUI/ci.yml?label=tests)](https://github.com/LlamaUI/LlamaUI/actions)

LlamaUI 是一个基于 Tauri 2.x 的桌面应用程序，为 llama-server（llama.cpp）提供图形化控制界面。它支持自动检测 llama-server 可执行文件和模型目录、服务进程管理、实时日志查看、性能监控等功能。

## 特性

- 🚀 **自动检测**：4 阶段优先级链自动查找 llama-server 和模型目录
- 🎛️ **三种模式**：普通模式（最简命令）、高级模式（完整参数）、专业模式（自定义命令）
- 📊 **实时监控**：CPU/内存/GPU 显存使用率、进程指标每 500ms 更新
- 🔒 **安全沙箱**：Windows Job Object 防孤儿进程、RCE 三重防护、URL scheme 白名单
- 🎨 **亮暗主题**：自动跟随系统或手动切换
- 📝 **日志管理**：实时流式日志、分组折叠、容量上限自动截断

## 快速开始

### 系统要求

- **Windows** 10/11（x86_64）
- **llama-server**（来自 [llama.cpp](https://github.com/ggerganov/llama.cpp/releases)）
- **模型文件**（.gguf 格式）

### 安装

1. 从 [Releases](https://github.com/LlamaUI/LlamaUI/releases) 下载最新版本的安装包
2. 运行安装程序完成安装
3. 首次启动时，应用会自动检测系统中的 llama-server 和模型目录

### 手动配置

如果自动检测未找到，可在设置中手动指定：

1. **llama-server 路径**：指向 `llama-server.exe` 可执行文件
2. **模型目录**：包含 .gguf 文件的文件夹路径

### 启动服务

1. 选择参数模式（普通/高级/专业）
2. 配置参数（高级模式）或自定义命令（专业模式）
3. 点击「启动」按钮
4. 在日志面板查看实时输出

## 参数模式

### 普通模式（Normal）

最简命令，最大兼容性。仅指定模型目录和端口，其它参数使用 llama-server 默认值。

```
llama-server --models-dir <目录> --port <端口> -ngl 99 --host 127.0.0.1
```

### 高级模式（Advanced）

完整参数控制，包括上下文大小、GPU 卸载层数、Flash Attention、MTP 多 token 预测等。

### 专业模式（Pro）

完全自定义启动命令，支持变量替换：

| 变量 | 说明 |
|------|------|
| `%%llama_server%%` | llama-server 可执行文件路径（自动加引号） |
| `%%models_dir%%` | 模型目录路径（自动加引号） |
| `%%port%%` | 当前端口 |
| `%%host%%` | 绑定地址（固定为 127.0.0.1） |

**示例**：
```
"%%llama_server%%" --models-dir "%%models_dir%%" --host %%host%% --port %%port%% -ngl all -c 32768 -fa on -ctk q5_0 -ctv q5_0 --spec-type draft-mtp --spec-draft-n-max 3 -tb 32
```

## 技术架构

```
┌─────────────────────────────────────────────────┐
│                   前端 (HTML/CSS/JS)             │
│                  dist/ (零构建)                  │
└─────────────────────┬───────────────────────────┘
                      │ Tauri IPC
┌─────────────────────▼───────────────────────────┐
│                  Rust 后端                       │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐         │
│  │ commands│  │ server  │  │ detect  │         │
│  │  (IPC)  │  │(进程管理)│  │(自动检测)│         │
│  └────┬────┘  └────┬────┘  └────┬────┘         │
│       │            │            │              │
│  ┌────▼────────────▼────────────▼────┐         │
│  │         共享层                      │         │
│  │  error / events / log / util / config│        │
│  └─────────────────────────────────────┘         │
└─────────────────────────────────────────────────┘
```

### 模块说明

| 模块 | 职责 |
|------|------|
| `commands` | Tauri IPC 命令适配层（21 个命令） |
| `server` | llama-server 进程管理（启动/停止/监控/日志） |
| `detect` | 自动检测（4 阶段优先级链） |
| `init` | 启动初始化（环境检查 → 驱动检查 → 自动加载） |
| `config` | 配置持久化（JSON + schema 版本迁移） |
| `backup` | 配置备份管理（创建/列出/恢复/删除） |
| `recovery` | 错误诊断与恢复建议 |
| `metrics_enhanced` | 增强版性能指标（滑动平均/趋势/峰值） |
| `error` | 统一错误类型（AppError + 子错误） |
| `events` | 事件名常量 + payload 类型（5 个事件） |
| `log` | 日志发射统一入口 |
| `util` | 通用工具（路径/时间/URL 白名单） |

## 安全特性

- **RCE 防护**：专业模式命令白名单校验（仅允许 llama-server/llama-cli/llama-bench/llama-embedding/llama-export）
- **进程隔离**：Windows Job Object 确保父进程死亡时自动回收子进程
- **URL 白名单**：外部链接仅允许 http/https scheme
- **PATH 注入防护**：拒绝从 tmp/temp/downloads 等世界可写目录加载可执行文件
- **内存安全**：日志行长度上限 16KB、缓冲容量上限 5000 行、通道背压丢弃

## 高级功能

### 日志导出

支持将运行日志导出为 txt/json/csv 格式：

1. 点击日志面板右上角的「导出」按钮
2. 选择导出格式（txt/json/csv）
3. 选择保存路径

### 配置备份

手动备份和恢复配置：

- **立即备份**：点击左侧面板底部的「立即备份」按钮
- **恢复备份**：在备份列表中选择要恢复的版本，点击「恢复」
- **删除备份**：点击备份列表中的「删除」按钮
- 系统自动保留最近 5 个备份，超出时自动清理

### 错误自动恢复

自动诊断配置问题并提供修复建议：

- 端口被占用时自动顺延到下一可用端口
- 模型目录不存在时引导用户重新检测
- llama-server 未找到时提供下载指引

### 性能监控增强

- **滑动平均**：5 次采样窗口，减少数据抖动
- **趋势指示**：箭头显示指标变化方向（↑↓→）
- **历史峰值**：记录本次运行期间的最大值

## 开发

### 环境要求

- Rust 1.70+
- Node.js（可选，仅用于前端开发）
- Visual Studio Build Tools（Windows）

### 构建

```bash
# 克隆仓库
git clone https://github.com/LlamaUI/LlamaUI.git
cd LlamaUI

# 调试构建
cargo build

# Release 构建
cargo build --release

# 运行测试
cargo test --lib

# Clippy 检查
cargo clippy --all-targets --release
```

### 项目结构

```
LlamaUI/
├── src/
│   ├── commands/          # Tauri IPC 命令
│   ├── server/            # 进程管理（9 个子模块）
│   ├── detect/            # 自动检测（5 个子模块）
│   ├── init/              # 启动初始化
│   ├── config.rs          # 配置持久化
│   ├── error.rs           # 统一错误类型
│   ├── events.rs          # 事件名 + payload
│   ├── log.rs             # 日志发射
│   ├── util/              # 通用工具
│   ├── main.rs            # 二进制入口
│   └── lib.rs             # Crate 根
├── dist/                  # 前端静态资源（零构建）
│   ├── index.html
│   ├── main.js
│   └── styles.css
├── icons/                 # 应用图标
├── capabilities/          # Tauri 权限配置
├── gen/schemas/           # Tauri 生成的 schema
├── Cargo.toml
├── tauri.conf.json
└── docs/
    ├── CODE_REVIEW_REPORT.md   # 代码审查报告
    └── REFACTORING.md          # 重构说明
```

## 测试

项目包含 115 个单元测试，覆盖关键安全逻辑：

```bash
cargo test --lib
# test result: ok. 115 passed; 0 failed
```

**测试覆盖**：
- RCE 防护（cmd/powershell/calc/llamainject 拒绝）
- Job Object 生命周期
- URL scheme 白名单
- 配置校验（端口/路径/数值范围）
- PATH 注入防护
- 并发取消检测
- 日志截断与容量管理

## 贡献

欢迎提交 Issue 和 Pull Request！

### 开发流程

1. Fork 本仓库
2. 创建功能分支（`feature/描述` 或 `fix/描述`）
3. 确保测试通过（`cargo test --lib`）
4. 确保 Clippy 无警告（`cargo clippy --all-targets --release`）
5. 提交 Pull Request

### 代码规范

- 遵循 `Cargo.toml` 中配置的 Clippy lint
- 生产代码禁止 `unwrap()` / `expect()` / `panic!`
- 错误消息使用中文
- 关键逻辑添加注释

## 许可证

MIT License - 详见 [LICENSE](LICENSE) 文件。

## 相关链接

- [llama.cpp](https://github.com/ggerganov/llama.cpp) - 底层 LLM 推理引擎
- [Tauri](https://tauri.app/) - 桌面应用框架
- [代码审查报告](docs/CODE_REVIEW_REPORT.md) - 完整技术审查
- [重构说明](docs/REFACTORING.md) - 架构设计文档
