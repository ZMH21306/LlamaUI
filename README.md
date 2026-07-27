# LlamaUI

一个轻量级的跨平台 UI 框架，帮助开发者快速构建桌面和 Web 应用。

<div align="center">

[![Forks](https://img.shields.io/github/forks/ZMH21306/LlamaUI?style=social)](https://github.com/ZMH21306/LlamaUI/fork)
[![Stars](https://img.shields.io/github/stars/ZMH21306/LlamaUI?style=social)](https://github.com/ZMH21306/LlamaUI/stargazers)
[![Issues](https://img.shields.io/github/issues/ZMH21306/LlamaUI)](https://github.com/ZMH21306/LlamaUI/issues)
[![Release](https://img.shields.io/github/v/release/ZMH21306/LlamaUI)](https://github.com/ZMH21306/LlamaUI/releases)
[![Downloads](https://img.shields.io/github/downloads/ZMH21306/LlamaUI/total)](https://github.com/ZMH21306/LlamaUI/releases)

</div>

[报告问题](https://github.com/ZMH21306/LlamaUI/issues/new?template=bug_report.yml) · [功能请求](https://github.com/ZMH21306/LlamaUI/issues/new?template=feature_request.yml)

## 关于项目

LlamaUI 致力于提供一个统一的 API，支持 Windows、Linux、macOS 三大平台的本地渲染，兼容 Web 技术栈。它的核心目标是 **零配置、即插即用**，让开发者专注业务逻辑。

### 主要功能

- ✅ 跨平台窗口管理
- ✅ 原生 UI 组件（按钮、输入框、表格）
- ✅ 支持 Web（React/Vue）和桌面双端渲染

### 技术栈

- **Rust** – 核心渲染引擎，提供高性能原生绘制
- **WebAssembly** – 将核心编译为 WASM，供前端框架调用
- **Tauri** – 桌面包装层，实现系统托盘、自动更新等功能

> **⚠️ 重要声明**
>
> 本项目遵循 MIT 许可证，代码仅供学习和商业使用，版权归原作者所有。请勿用于侵权或违法用途。

## 下载与兼容性

为获得最佳兼容性，请使用最新发布的二进制文件。

| 平台 | 最低要求 | 架构 | 兼容性 | 下载链接 |
|------|----------|------|--------|----------|
| **Windows** | Windows 10 1809+ | x86_64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/latest/download/llamaui-windows-x86_64.exe) |
| **Windows** | Windows 10 1809+ | arm64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/latest/download/llamaui-windows-arm64.exe) |
| **Linux** | glibc 2.28+ | x86_64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/latest/download/llamaui-linux-x86_64) |
| **Linux** | glibc 2.28+ | arm64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/latest/download/llamaui-linux-arm64) |
| **macOS** | macOS 11+ | x86_64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/latest/download/llamaui-macos-x86_64) |
| **macOS** | macOS 11+ | arm64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/latest/download/llamaui-macos-arm64) |

## 开发路线

### ✅ 已完成功能

- ✅ 跨平台窗口管理
- ✅ 基础 UI 组件实现

### 🚧 计划中功能

- [ ] 深色模式支持
- [ ] 国际化（i18n）

### 🐛 已知问题

- 部分 Linux 发行版在图形加速上存在兼容性问题 – 正在调查中

## 贡献指南

贡献让开源社区更加活跃。欢迎提交 Pull Request 或者打开带有 `enhancement` 标签的 Issue。

1. Fork 本仓库
2. 创建功能分支 (`git checkout -b feature/AmazingFeature`)
3. 提交更改 (`git commit -m 'Add AmazingFeature'`)
4. 推送到分支 (`git push origin feature/AmazingFeature`)
5. 打开 Pull Request

> 别忘了给项目点个星星 ⭐！

## 许可证

依据 MIT 许可证分发。更多信息请参阅 `LICENSE` 文件。

版权所有 © 2026 ZMH21306

## 联系方式

- [电子邮件](mailto:example@example.com) - example@example.com
- QQ 群 - 12345678
- 其他联系方式请自行补充

## Star 历史

[![Star History Chart](https://api.star-history.com/svg?repos=ZMH21306/LlamaUI&type=Date)](https://star-history.com/#ZMH21306/LlamaUI&Date)
