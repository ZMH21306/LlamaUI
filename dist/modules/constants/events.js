/**
 * 前后端事件契约常量集
 *
 * 与 Rust `events.rs` 中的 EVT_* 常量严格一一对应。前端任何事件名称必须
 * 从此文件取值，避免拼写错误导致事件监听静默失效（这是旧版最大的隐患之一）。
 *
 * 分类：
 *  - 后端→前端 IPC 事件：Rust emit_event 发出，前端 bus.on 监听
 *  - 内部 UI 事件：仅前端总线流转，不经过 IPC
 */
export const EVT = Object.freeze({
  // ===== 服务生命周期 =====
  SERVER_LOG: 'server-log',
  SERVER_STATUS: 'server-status',
  SERVER_METRICS: 'server-metrics',

  // ===== 自动检测 / 初始化流程 =====
  DETECT_STEP: 'detect-step',

  // ===== 下载（llama-downloader / hf-downloader）=====
  DOWNLOAD_STATE: 'download-state',
  DOWNLOAD_PROGRESS: 'download-progress',

  // ===== 更新流程 =====
  UPDATE_STATE: 'update-state',
  UPDATE_PROGRESS: 'update-progress',

  // ===== 硬件 / 模型 =====
  GPU_INFO: 'gpu-info',
  MODEL_SELECTED: 'model-selected',

  // ===== HF 模型商店（独立窗口模块，跨窗口通信）=====
  HF_DOWNLOAD_PROGRESS: 'hf-download-progress',

  // ===== 内部 UI 事件（仅前端总线，不经过 IPC）=====
  // 服务控制意图
  UI_START_REQUESTED: 'ui:start-requested',
  UI_STOP_REQUESTED: 'ui:stop-requested',
  UI_RESTART_REQUESTED: 'ui:restart-requested',
  // 配置
  UI_CONFIG_DIRTY: 'ui:config-dirty',
  // 下载/更新意图
  UI_DOWNLOAD_REQUESTED: 'ui:download-requested',
  UI_DOWNLOAD_CANCEL: 'ui:download-cancelled',
  UI_UPDATE_CHECK: 'ui:update-check',
  UI_INIT_REQUESTED: 'ui:init-requested',
  // UI 行为
  UI_THEME_CHANGE: 'ui:theme-change',
  UI_SPLITTER_RESIZE: 'ui:splitter-resize',
});

export default EVT;