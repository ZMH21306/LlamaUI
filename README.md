# LlamaUI

基于 Llama.cpp 的轻量级 GUI 前端程序，提供直观的界面来管理和运行本地 LLM 推理服务。

## 功能特性

- **一键启动**：自动检测本地 Llama.cpp 服务，快速启动推理服务
- **智能配置**：自动识别模型目录，支持多种参数配置
- **实时监控**：实时显示服务器日志、性能指标（CPU/内存/显存）
- **进程保护**：使用 Windows Job Object 确保子进程在异常情况下被正确终止
- **多模式支持**：支持 Normal、Advanced、Pro 三种配置模式

## 系统要求

- Windows 10/11 (x86_64)
- WebView2 Runtime（Windows 11 默认已安装，Windows 10 可[下载](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)）
- Llama.cpp 可执行文件（可选，自动检测）

## 快速开始

### 预编译版本

从 [Releases](https://github.com/your-repo/LlamaUI/releases) 下载最新版本，解压后运行 `LlamaUI.exe`。

### 从源码构建

```bash
# 1. 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# 或访问 https://rustup.rs

# 2. 安装 Node.js (v18+)
# https://nodejs.org/

# 3. 克隆项目
git clone https://github.com/your-repo/LlamaUI.git
cd LlamaUI

# 4. 安装前端依赖
npm install

# 5. 构建项目
cargo build --release

# 6. 运行
cargo run --release
```

构建完成后，可执行文件位于 `target/release/llama-ui.exe`。

## 项目结构

```
LlamaUI/
├── src/                    # Rust 后端源码
│   ├── commands/           # Tauri IPC 命令层
│   ├── detect/             # Llama.cpp 自动检测模块
│   ├── init/               # 启动初始化模块
│   ├── server/             # 进程管理模块
│   ├── util/               # 工具函数
│   └── main.rs             # 入口点
├── dist/                   # 前端静态资源
├── icons/                  # 应用图标
├── Cargo.toml              # Rust 依赖配置
└── tauri.conf.json         # Tauri 应用配置
```

## 配置说明

首次运行时会自动检测系统环境并生成配置文件，存储于 `%APPDATA%\LlamaUI\config.json`。

### 主要配置项

| 配置项 | 说明 | 默认值 |
|--------|------|--------|
| `llama_server_path` | Llama.cpp 服务端路径 | 自动检测 |
| `models_dir` | 模型文件目录 | 自动检测 |
| `port` | 服务端口 | 8080 |
| `ctx_size` | 上下文大小 | 512 |

## 开发

### 运行开发版本

```bash
cargo run
```

### 运行测试

```bash
cargo test --lib
```

### 代码检查

```bash
cargo clippy --all-targets --release
```

## 许可证

本项目采用 [MIT 许可证](LICENSE)。