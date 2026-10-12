/**
 * 本地化文案（当前仅 zh-CN，预留 i18n 扩展点）
 * 统一从单一来源取文案，避免文案硬编码分散在各类逻辑中。
 */
export const M = Object.freeze({
  // —— 服务状态 ——
  serviceStarting: '启动中',
  serviceRunning: '运行中',
  serviceStopping: '停止中',
  serviceStopped: '已停止',
  serviceCrashed: '服务崩溃',
  serviceUnknown: '未知状态',
  serviceUptime: '运行时长',

  // —— 按钮 ——
  btnStart: '启动服务',
  btnStartLoading: '启动中',
  btnStop: '停止服务',
  btnStopLoading: '停止中',
  btnRestart: '重启服务',
  btnRestartLoading: '重启中',

  // —— 配置 ——
  configSaved: '配置已保存',
  configSaving: '正在保存',
  configSaveFailed: '保存配置失败',
  configDirty: '有未保存的更改',
  configSaveTime: '已保存 {time}',
  configRestoreSuccess: '配置已加载',
  configRestoreFailed: '加载配置失败',

  // —— 下载 ——
  downloadStarting: '准备下载',
  downloadRunning: '下载中',
  downloadComplete: '下载完成',
  downloadFailed: '下载失败',
  downloadCancelled: '已取消',
  downloadError: '下载出错',
  downloadInstall: '安装中',
  downloadInstallComplete: '安装完成',
  downloadInstallFailed: '安装失败',

  // —— 更新 ——
  updateChecking: '检查更新中',
  updateAvailable: '发现新版本 {version}',
  updateReadyInstall: '可以安装新版本',
  updateInstalling: '安装中',
  updateComplete: '更新完成，需要重启应用',
  updateFailed: '更新失败',
  updateCancelled: '已取消',

  // —— 自动检测/初始化 ——
  initChecking: '检查中',
  initCompleted: '环境就绪',
  initFailed: '环境检查失败',

  // —— 通用 ——
  ok: '确定',
  cancel: '取消',
  confirm: '确定',
  retry: '重试',
  loading: '加载中',
  empty: '（无）',
  unknown: '未知',
  none: '无',
  error: '错误',
  warning: '警告',
  info: '信息',

  // —— 指标 ——
  metricUnavailable: '不可用',
});

export default M;

/** 相对时间格式化（ms → 字符串） */
export function formatElapsedMs(ms) {
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const ss = Math.floor(s % 60);
  return `${m}分${ss}秒`;
}

/** 绝对时间格式化（Date | number → HH:MM:SS） */
export function formatTime(date) {
  const d = new Date(date);
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  const ss = String(d.getSeconds()).padStart(2, '0');
  return `${hh}:${mm}:${ss}`;
}