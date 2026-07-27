# LlamaUI

轻量级 GUI 前端，用于管理和运行本地 Llama.cpp 推理服务。

<div align="center">
[![GitHub forks](https://img.shields.io/github/forks/ZMH21306/LlamaUI?style=social)](https://github.com/ZMH21306/LlamaUI/fork)
[![GitHub stars](https://img.shields.io/github/stars/ZMH21306/LlamaUI?style=social)](https://github.com/ZMH21306/LlamaUI/stargazers)
[![GitHub issues](https://img.shields.io/github/issues/ZMH21306/LlamaUI)](https://github.com/ZMH21306/LlamaUI/issues)
[![GitHub release](https://img.shields.io/github/v/release/ZMH21306/LlamaUI)](https://github.com/ZMH21306/LlamaUI/releases)
[![GitHub downloads](https://img.shields.io/github/downloads/ZMH21306/LlamaUI/total)](https://github.com/ZMH21306/LlamaUI/releases)
</div>

[报告问题](https://github.com/ZMH21306/LlamaUI/issues/new) · [功能请求](https://github.com/ZMH21306/LlamaUI/issues/new)

## 关于项目

LlamaUI 是基于 **Llama.cpp** 的跨平台 GUI 前端，提供直观的界面帮助用户快速启动、配置和监控本地大语言模型推理服务。

### 主要功能

- ✅ 一键启动 Llama.cpp 推理服务
- ✅ 自动检测模型目录并生成默认配置
- ✅ 实时日志、CPU/显存监控
- ✅ 多模式（Normal、Advanced、Pro）参数切换
- ✅ 进程保护，异常退出自动回收子进程

### 技术栈

- [Tauri](https://tauri.studio/) – 跨平台桌面应用框架（Rust + Web 前端）
- [Rust](https://www.rust-lang.org/) – 后端业务逻辑、进程管理
- [Llama.cpp](https://github.com/ggerganov/llama.cpp) – 本地 LLM 推理引擎
- [React (Vite)](https://vitejs.dev/) – 前端 UI

> **⚠️ 重要声明**
> 
> 本项目仅供学习与研究使用，使用时请遵守所在地区的法律法规以及模型的授权协议。

## 下载与兼容性

为获得最佳兼容性，请使用最新发布的版本。

| 平台 | 最低要求 | 架构 | 兼容性 | 下载链接 |
|------|----------|------|--------|----------|
| **Windows** | Windows 10 1809+ | x86_64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/download/v0.1.0/LlamaUI-windows-x86_64.zip) |
| **Windows** | Windows 10 1809+ | arm64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/download/v0.1.0/LlamaUI-windows-arm64.zip) |
| **Linux** | glibc 2.28+ | x86_64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/download/v0.1.0/LlamaUI-linux-x86_64.tar.gz) |
| **Linux** | glibc 2.28+ | arm64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/download/v0.1.0/LlamaUI-linux-arm64.tar.gz) |
| **macOS** | macOS 11+ | x86_64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/download/v0.1.0/LlamaUI-macos-x86_64.dmg) |
| **macOS** | macOS 11+ | arm64 | ✅ | [GitHub 直链](https://github.com/ZMH21306/LlamaUI/releases/download/v0.1.0/LlamaUI-macos-arm64.dmg) |

## 开发路线

### ✅ 已完成功能

- ✅ 一键启动 Llama.cpp
- ✅ 实时日志与资源监控

### 🚧 计划中功能

- [ ] 多模型管理 UI
- [ ] 插件系统支持自定义扩展

### 🐛 已知问题

- 暂无已知问题

## 贡献指南

贡献让开源社区成为学习、启发和创造的舞台。我们**非常感谢**您的每一次贡献。

1. Fork 本仓库
2. 创建功能分支 (`git checkout -b feature/YourFeature`)
3. 提交更改 (`git commit -m "feat: add YourFeature"`)
4. 推送分支 (`git push origin feature/YourFeature`)
5. 打开 Pull Request

> 别忘了给项目点个 ⭐！

## 许可证

本项目采用 **MIT 许可证**。详见 `LICENSE` 文件。

版权所有 © 2026 ZMH21306

## 联系方式

- [电子邮箱](mailto:zhangmh21306@example.com)
- QQ 群：123456789

## Star 历史

[![Star History Chart](https://api.star-history.com/svg?repos=ZMH21306/LlamaUI&type=Date)](https://star-history.com/#ZMH21306/LlamaUI&Date)