# 更新日志

本文件记录 LlamaUI 项目的所有重要变更。格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.6.0] - 2026-08-14

### 新增
- 引入 CONTRIBUTING.md、SECURITY.md 等开源协作文档
- 整理项目目录，移除冗余内部脚本

## [0.5.1] - 2026-08-08

### 改进
- **llama-server 下载重构**：改用 curl 子进程发起 HTTP 请求，自动继承系统代理和 TLS 配置
- **GitHub Token 认证**：支持 GITHUB_TOKEN / gh CLI / GH_TOKEN 多来源认证，解决 API 限流
- **更新检查优化**：重构 GitHub Releases API 调用，提升稳定性和错误处理
- **下载命令优化**：改进下载进度与错误处理逻辑

### 修复
- 修复下载与更新检查相关的稳定性问题

## [0.5.0] - 2026-08-02

### 新增
- **GPU 自动检测**：支持 NVIDIA/AMD/Intel GPU 型号、驱动版本、CUDA/ROCm 版本检测
- **llama-server 自动下载**：一键从 GitHub Releases 下载适配当前 GPU 后端的 llama.cpp 预编译包
- **GPU 后端自动选择**：根据 GPU 类型自动选择 CUDA/ROCm/Vulkan/Metal/CPU 后端
- **全局日志系统**：基于 tracing 生态，控制台彩色分级输出 + 文件持久化（按日滚动）
- **Panic Hook**：崩溃时自动写入完整 backtrace 到日志文件
- **前端 GPU 信息卡片**：实时显示 GPU 型号、显存、驱动版本、推荐后端
- **前端下载按钮**：支持 llama-server 一键下载、进度条、SHA256 校验
- **前端检查更新按钮**：顶栏手动检查更新入口

### 改进
- 更新检查扩展支持 Linux/macOS 旧版本路径检测
- NVIDIA GPU 检测不依赖 CUDA 版本解析（nvidia-smi 成功即推荐 CUDA）
- GitHub API 请求添加 Accept header 解决 403 问题
- 前端下载进度平滑动画（每帧最多 +2% 渐进）

### 修复
- tracing_setup.rs 编译错误（缺少 Layer/SubscriberExt/SubscriberInitExt 导入）
- 移除未使用的 tracing-error 依赖

## [0.4.0] - 2026-08-02

### 新增
- **日志导出**：支持将运行日志导出为 txt/json/csv 格式
- **配置备份**：手动备份、自动保留最近 5 个备份、一键恢复/删除
- **错误自动恢复**：诊断配置问题并提供修复建议（端口占用自动顺延）
- **性能监控增强**：滑动平均（5 次采样窗口）、趋势指示箭头、历史峰值追踪
- **自动更新检查**：通过 GitHub Releases API 检查最新版本，检测旧版本残留并提示清理

### 改进
- `Metrics` 结构体实现 `Default` trait，支持默认值初始化
- 前端备份面板集成到左侧配置面板底部
- 诊断结果支持序列化，可通过 IPC 传递给前端

### 技术变更
- 新增模块：`backup`（配置备份管理）、`recovery`（错误诊断）、`metrics_enhanced`（增强指标）、`update_check`（自动更新检查）
- 新增 IPC 命令：`export_logs`、`create_config_backup`、`list_config_backups`、`restore_config_backup`、`delete_config_backup`、`get_diagnosis`、`auto_fix_issues`、`check_updates`、`cleanup_old_version`
- 单元测试从 115 个增加到 139 个
- 新增依赖：`ureq`（HTTP 客户端，用于检查更新）
- CI/CD 工作流升级：构建矩阵扩展至 7 平台（Windows x64/x86/arm64、Linux x64/arm64、macOS x64/arm64），产物统一命名为 `LlamaUI_<版本>_<系统>_<架构>.<后缀>`，Release Notes 按标准格式自动生成

## [0.3.0] - 2026-07-31

### 新增
- 专业模式（Pro）自定义命令支持，允许完全自定义 llama-server 启动参数
- 命令变量替换：`%%llama_server%%`、`%%models_dir%%`、`%%port%%`、`%%host%%`
- 自动端口顺延功能（`auto_port` 配置项）
- 配置 schema 版本控制与自动迁移机制
- 115 个单元测试，覆盖关键安全逻辑

### 改进
- 完整重构后端模块架构，单向依赖：error/events/log/util → config → server/detect → init → commands
- 统一错误类型：`AppError` 顶层 + 子错误（`ConfigError` / `ProcessError` / `DetectError`）
- 事件名集中管理，编译期发现不一致
- 日志发射统一入口（`crate::log`）
- 检测流程 4 阶段优先级链，带时间预算与取消支持
- 进程生命周期三重保险：Windows Job Object + tokio `kill_on_drop` + panic hook

### 安全
- RCE 三重防护：专业模式命令白名单校验
- URL scheme 白名单：仅允许 http/https
- PATH 注入防护：拒绝从 tmp/temp/downloads 加载可执行文件
- 内存安全：日志行上限 16KB、缓冲 5000 行、通道容量 2048、背压丢弃

### 修复
- P0-3：5 个后台任务原子注册，避免 partial registration 期间 Drop 只能 abort 部分
- P0-6：有界 mpsc 日志通道，溢出丢弃而非 OOM
- P1-009：Drop 兜底必须清空 pid/started_at/active_port/status
- P1-1：Job Object 使用真句柄（`OpenProcess` 而非 `raw_handle` 伪句柄）
- P2-9：pro 模式 `custom_command` 解析用 let-else 替代 unwrap

## [0.2.0] - 2026-07-09

### 新增
- 高级模式（Advanced）：完整参数控制（上下文大小、GPU 卸载层数、Flash Attention、MTP）
- 亮/暗色主题切换
- 实时性能监控（CPU/内存/GPU 显存）
- 自动检测 llama-server 和模型目录（4 阶段优先级链）
- 启动初始化三步流程（环境检查 → 驱动检查 → 自动加载）

### 改进
- 端口探测并行化（100 端口 < 2s）
- sysinfo 全局缓存，避免内存抖动
- GPU 查询缓存（5s TTL）
- 日志 log pump 借用优化

## [0.1.0] - 2026-07-09

### 新增
- 初始版本发布
- 普通模式（Normal）：最简命令启动 llama-server
- 服务进程控制（启动/停止/重启/状态查询）
- 实时日志流式查看
- 配置持久化（JSON 格式）
- Tauri 2.x 桌面应用框架

---

## 版本说明

### 语义化版本

- **_major_**：破坏性变更（配置 schema 变更、API 不兼容）
- **minor_**：新功能（向后兼容）
- **patch_**：Bug 修复（向后兼容）

### 配置迁移

配置 schema 版本通过 `_v` 字段管理。旧版本配置文件会在加载时自动迁移到当前版本，迁移失败时退回默认值。

当前配置版本：`v1`
