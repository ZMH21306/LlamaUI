# 更新日志

本文件记录 LlamaUI 项目的所有重要变更。格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

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
