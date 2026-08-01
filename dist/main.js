// ============================================================
// LlamaUI 前端主脚本
// 结构：常量 → 状态 → DOM 缓存 → 工具 → 业务模块 → 事件 → 启动
// 设计原则：单状态源、防御性编程、事件委托、rAF 合并 DOM 写入
// ============================================================
'use strict';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;

// ============= 常量 =============
const STATUS_TEXT = { Stopped: '已停止', Starting: '启动中', Running: '运行中', Crashed: '已崩溃' };
const STEP_STATUS_TEXT = { pending: '等待中', running: '进行中…', success: '✓ 完成', failed: '✗ 失败' };
const TOAST_ICON = { success: '✓', error: '✕', warning: '!', info: 'i' };
const TOAST_DURATION = 3000;
const PLAIN_GROUP_ID = '_plain';
const MAX_LOG_LINES_PER_GROUP = 5000;
const SAVE_DEBOUNCE_MS = 300;
const DEFAULT_PORT = 10897;
const DEFAULT_PROGRAM = 'llama-server';
const MIN_PANE_W = 240;
const MAX_PANE_LEFT_W = 720;
const MAX_PANE_RIGHT_W = 800;

// ============= 状态 =============
const state = {
  status: 'Stopped',
  mode: 'normal',
  port: DEFAULT_PORT,
  activePort: null,
  saving: false,
  // 主题：默认暗色，false = 暗色, true = 亮色
  lightTheme: false,
  // 日志分组：id -> { id, name, status, expanded, groupEl, contentEl, arrowEl, statusEl }
  groups: {},
  groupOrder: [],
};

// ============= DOM 缓存 =============
const $ = (id) => document.getElementById(id);
const $$ = (sel) => document.querySelectorAll(sel);

/**
 * 安全解析 URL。失败时返回 null（如 about:blank、空字符串）。
 */
function tryParseUrl(s) {
  if (!s || s === 'about:blank') return null;
  try { return new URL(s); } catch { return null; }
}

/**
 * 把 URL 规范化为可比较的字符串：去 fragment、去末尾 /、端口归一。
 * 用于判断 iframe 当前 src 与目标 URL 是否需要重置。
 */
function normalizeUrl(u) {
  if (!u) return '';
  let s = u.origin + u.pathname.replace(/\/+$/, '') + u.search;
  // http 默认端口 80 视同 80
  if ((u.protocol === 'http:' && u.port === '80') ||
      (u.protocol === 'https:' && u.port === '443')) {
    s = u.protocol + '//' + u.hostname + u.pathname.replace(/\/+$/, '') + u.search;
  }
  return s;
}

const els = {
  // 顶栏
  statusPill: $('statusPill'),
  statusText: document.querySelector('.status-text'),
  startBtn: $('startBtn'),
  stopBtn: $('stopBtn'),
  restartBtn: $('restartBtn'),

  // 通用确认弹窗
  modal: $('modal'),
  modalTitle: $('modalTitle'),
  modalBody: $('modalBody'),
  modalConfirm: $('modalConfirm'),
  modalCancel: $('modalCancel'),

  // 路径
  llamaServerPath: $('llamaServerPath'),
  browseLlamaServer: $('browseLlamaServer'),
  detectLlamaServer: $('detectLlamaServer'),
  modelsDir: $('modelsDir'),
  browseModelsDir: $('browseModelsDir'),
  detectModelsDir: $('detectModelsDir'),

  // 模式
  modeTabs: $$('.mode-tab'),
  modeViews: $$('.mode-view'),

  // 普通模式
  port: $('port'),
  autoPort: $('autoPort'),
  normalCmdPreview: $('normalCmdPreview'),

  // 高级模式
  advancedAccordion: $('advancedAccordion'),
  advancedPreview: $('advancedPreview'),
  advancedExpandAll: $('advancedExpandAll'),
  advancedCollapseAll: $('advancedCollapseAll'),
  advancedReset: $('advancedReset'),

  // 专业模式
  customCommand: $('customCommand'),
  varGrid: $('varGrid'),

  // 监控
  metricPid: $('metricPid'),
  metricUptime: $('metricUptime'),
  metricCpuText: $('metricCpuText'),
  metricCpuBar: $('metricCpuBar'),
  metricMemVirtText: $('metricMemVirtText'),
  metricMemVirtBar: $('metricMemVirtBar'),
  metricVramText: $('metricVramText'),
  metricVramBar: $('metricVramBar'),
  metricGpuText: $('metricGpuText'),
  metricGpuBar: $('metricGpuBar'),

  // 日志
  logs: $('logs'),
  clearLogs: $('clearLogs'),
  exportLogs: $('exportLogs'),
  autoScroll: $('autoScroll'),

  // 配置导入/导出
  exportConfig: $('exportConfig'),
  importConfig: $('importConfig'),

  // WebView
  webviewPlaceholder: $('webviewPlaceholder'),
  webviewLoading: $('webviewLoading'),
  loadingUrl: $('loadingUrl'),
  openInBrowser: $('openInBrowser'),
  openInBrowserToolbar: $('openInBrowserToolbar'),
  reloadWebview: $('reloadWebview'),
  stopFromLoading: $('stopFromLoading'),
  webview: $('webview'),
  placeholderUrl: $('placeholderUrl'),

  // 分隔条
  splitter: $('splitter'),
  splitterRight: $('splitterRight'),

  // 通知
  toastContainer: $('toastContainer'),
  // 主题切换
  themeToggle: $('themeToggle'),
  // 配置预设
  configTemplateSelect: $('configTemplateSelect'),
};

// ============= 工具函数 =============
function now() {
  const d = new Date();
  const p = (n, l = 2) => String(n).padStart(l, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ` +
         `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

function debounce(fn, ms) {
  let t = null;
  return function (...args) {
    if (t) clearTimeout(t);
    t = setTimeout(() => fn.apply(this, args), ms);
  };
}

/** 校验端口号。返回 { ok: bool, value: number, reason?: string }。
 *  - 接受纯数字字符串
 *  - 范围 1 ~ 65535
 *  - 拒绝科学计数法（如 1e5）、含空格、含负号、含非数字字符
 */
function validatePort(raw) {
  if (raw == null) return { ok: false, value: NaN, reason: '空值' };
  const s = String(raw).trim();
  if (s === '') return { ok: false, value: NaN, reason: '空值' };
  if (!/^\d+$/.test(s)) return { ok: false, value: NaN, reason: '含非数字字符' };
  const n = Number(s);
  if (!Number.isFinite(n)) return { ok: false, value: NaN, reason: '无法解析为数字' };
  if (n < 1 || n > 65535) return { ok: false, value: n, reason: '范围 1~65535' };
  return { ok: true, value: n };
}

/** 显示输入框错误样式（红边 + 抖动）。type=number 的输入框仅支持 type=number，
 *  通过加 class 来标错。3 秒后自动清除。 */
function flashInputError(el, reason) {
  if (!el) return;
  el.classList.add('input-error');
  el.title = reason || '输入无效';
  if (el._errTimer) clearTimeout(el._errTimer);
  el._errTimer = setTimeout(() => {
    el.classList.remove('input-error');
    el.title = '';
  }, 3000);
}

/** 同步端口输入框的视觉状态。返回与 validatePort 一致的 { ok, value, reason } 形状，
 * 这样所有调用方都能用 `.ok` 字段统一判断。
 *   - ok: true  表示端口合法
 *   - ok: false 表示端口非法（已标红+抖动+ tooltip）
 */
function syncPortInput() {
  if (!els.port) return { ok: true, value: DEFAULT_PORT };
  const r = validatePort(els.port.value);
  if (!r.ok) {
    flashInputError(els.port, `端口无效：${r.reason}`);
    return { ok: false, value: NaN, reason: r.reason };
  }
  els.port.classList.remove('input-error');
  els.port.title = '';
  return { ok: true, value: r.value };
}

function safeCall(fn, fallback = null) {
  try { return fn(); } catch (e) { console.error(e); return fallback; }
}

// 等待 status 变成 target 之一（轮询）。timeout 毫秒后强制返回。
async function waitForStatus(targets, timeout = 4000) {
  const t0 = Date.now();
  const want = Array.isArray(targets) ? targets : [targets];
  while (Date.now() - t0 < timeout) {
    if (want.includes(state.status)) return true;
    await new Promise((r) => setTimeout(r, 80));
  }
  return want.includes(state.status);
}

function formatUptime(secs) {
  secs = Math.max(0, secs | 0);
  if (secs === 0) return '0秒';
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h) return `${h}小时${m}分${s}秒`;
  if (m) return `${m}分${s}秒`;
  return `${s}秒`;
}

function setMeter(barEl, pct, opts = {}) {
  if (!barEl) return;
  barEl.classList.remove('warn', 'danger', 'unavailable');
  if (opts.unavailable) {
    barEl.classList.add('unavailable');
    return;
  }
  const w = Math.max(0, Math.min(100, +pct || 0));
  barEl.style.width = `${w}%`;
  if (opts.danger != null && w >= opts.danger) barEl.classList.add('danger');
  else if (opts.warn != null && w >= opts.warn) barEl.classList.add('warn');
}

// ============= 顶部通知（Toast）=============
// 最近一次 toast 的 key+时间戳：用于去重（避免「检测路径」连续点 5 次
// 出现 5 个相同 toast）。相同 (text, type) 在 DEDUP_WINDOW_MS 内只保留一个。
const DEDUP_WINDOW_MS = 1500;
let _lastToastKey = null;
let _lastToastAt = 0;

function showNotification(text, type = 'info', duration = TOAST_DURATION) {
  if (!els.toastContainer) return;
  const key = `${type}|${text}`;
  const now = Date.now();
  if (_lastToastKey === key && now - _lastToastAt < DEDUP_WINDOW_MS) {
    // 命中去重窗口：刷新最旧那条的关闭定时器，但不再追加新 DOM
    return;
  }
  _lastToastKey = key;
  _lastToastAt = now;
  const toast = document.createElement('div');
  toast.className = `toast ${type}`;
  toast.innerHTML = `
    <span class="toast-icon">${TOAST_ICON[type] || 'i'}</span>
    <span class="toast-text"></span>
    <button class="toast-close" type="button" aria-label="关闭">×</button>
  `;
  toast.querySelector('.toast-text').textContent = text;
  els.toastContainer.appendChild(toast);
  // 下一帧触发动画
  requestAnimationFrame(() => toast.classList.add('show'));
  let removed = false;
  const remove = () => {
    if (removed) return;
    removed = true;
    toast.classList.remove('show');
    setTimeout(() => toast.remove(), 220);
  };
  toast.querySelector('.toast-close').addEventListener('click', remove);
  if (duration > 0) setTimeout(remove, duration);
}

// 通用确认弹窗。返回 Promise<boolean>，true = 用户点确定。
function showConfirm({ title = '确认', body = '', confirmText = '确定', cancelText = '取消' } = {}) {
  return new Promise((resolve) => {
    if (!els.modal || !els.modalBody || !els.modalTitle || !els.modalConfirm) {
      resolve(window.confirm(`${title}\n${body}`));
      return;
    }
    els.modalTitle.textContent = title;
    els.modalBody.textContent = body;
    els.modalConfirm.textContent = confirmText;
    els.modalCancel.textContent = cancelText;
    els.modal.hidden = false;
    let settled = false;
    const cleanup = (val) => {
      if (settled) return;
      settled = true;
      els.modal.hidden = true;
      els.modalConfirm.removeEventListener('click', onOk);
      els.modalCancel.removeEventListener('click', onCancel);
      els.modal.querySelectorAll('[data-modal-close]').forEach((el) =>
        el.removeEventListener('click', onCancel)
      );
      document.removeEventListener('keydown', onKey);
      resolve(val);
    };
    const onOk = () => cleanup(true);
    const onCancel = () => cleanup(false);
    const onKey = (e) => {
      if (e.key === 'Escape') onCancel();
      else if (e.key === 'Enter') onOk();
    };
    els.modalConfirm.addEventListener('click', onOk);
    els.modalCancel.addEventListener('click', onCancel);
    els.modal.querySelectorAll('[data-modal-close]').forEach((el) =>
      el.addEventListener('click', onCancel)
    );
    document.addEventListener('keydown', onKey);
    setTimeout(() => els.modalConfirm.focus(), 0);
  });
}

// ============= 配置读写 =============
function readConfigFromUI() {
  let extraArgs = '';
  if (state.mode === 'advanced' && els.advancedAccordion?.dataset.rendered) {
    extraArgs = collectAdvancedAsExtraArgs();
  }
  // 端口优先做本地校验：无效时回退默认值（同时 UI 已显示错误样式）。
  // 后端仍会做最终校验（防止 Tauri 直接调用绕过 UI），并拒绝非法值。
  const portCheck = validatePort(els.port.value);
  const port = portCheck.ok ? portCheck.value : DEFAULT_PORT;
  // 模型目录白名单校验：含 NUL 字符时直接拒绝保存（后端也会拒绝）
  const modelsDirRaw = els.modelsDir.value.trim();
  const modelsDir = modelsDirRaw.includes('\0') ? '' : modelsDirRaw;
  // 自定义 llama-server 路径白名单：含 NUL 字符时清空（不保存危险值）
  const llamaPathRaw = (els.llamaServerPath.value || '').trim();
  const llamaPath = llamaPathRaw.includes('\0') ? null : (llamaPathRaw || null);
  // 专业模式命令：含 NUL 字符时清空
  const customCmd = els.customCommand?.value || '';
  const safeCustomCmd = customCmd.includes('\0') ? '' : customCmd;
  return {
    llama_server_path: llamaPath,
    models_dir: modelsDir,
    // 普通模式不使用以下参数，保留以兼容旧配置；启动时由后端按 mode 决定是否使用
    ctx_size: 4096,
    n_gpu_layers: -1,
    flash_attn: false,
    mtp: false,
    mtp_draft_n_max: 3,
    port,
    auto_port: els.autoPort.checked,
    extra_args: extraArgs,
    mode: state.mode,
    custom_command: safeCustomCmd,
  };
}

function writeConfigToUI(cfg) {
  if (!cfg) return;
  els.llamaServerPath.value = cfg.llama_server_path || '';
  els.modelsDir.value = cfg.models_dir || '';
  // 加载时如果后端给的是非法 port（理论上不会，但旧配置/手动改文件可能），
  // 用本地校验强制恢复为默认值，避免「显示 0 但启动报无效」的迷惑状态。
  const portCheck = validatePort(cfg.port);
  els.port.value = portCheck.ok ? String(portCheck.value) : String(DEFAULT_PORT);
  els.autoPort.checked = cfg.auto_port !== false;
  if (els.customCommand) els.customCommand.value = cfg.custom_command || '';
  setMode(cfg.mode || 'normal', { skipRender: true });
  renderProVars();
  updatePlaceholderUrl();
  refreshAllPreviews();
  // 加载后做一次 pro 模式命令实时校验
  if (state.mode === 'pro') validateProCommandLive();
}

const scheduleSave = debounce(async () => {
  if (state.saving) return;
  state.saving = true;
  try {
    await invoke('save_config', { config: readConfigFromUI() });
  } catch (e) {
    showNotification(`保存配置失败：${e}`, 'error');
  } finally {
    state.saving = false;
  }
}, SAVE_DEBOUNCE_MS);

// ============= 状态 / WebView =============
function updateStatusUI(status) {
  if (!status || !STATUS_TEXT[status]) return;
  state.status = status;
  els.statusPill.dataset.status = status;
  els.statusText.textContent = STATUS_TEXT[status];
  const running = status === 'Running' || status === 'Starting';
  els.startBtn.disabled = running;
  els.stopBtn.disabled = !running;
  // 重启按钮：仅在服务处于"运行中"时可点；Stopped/Crashed 时禁用（需先启动才能重启）
  els.restartBtn.disabled = status !== 'Running';
  if (status === 'Stopped' || status === 'Crashed') {
    state.activePort = null;
    resetMetrics();
  }
  updateWebviewVisibility();
}

// ============= 主题切换 =============
/**
 * 同步 iframe 配色与软件主题保持一致。
 * 由于 iframe 内是第三方 llama.cpp WebUI（跨源），无法用 CSS 注入或 DOM 操作，
 * 采用两层联动：
 *   1) `color-scheme` 属性：透传给 iframe，影响 UA 控件（滚动条、原生表单）
 *      及较新 llama.cpp 的 `@media (prefers-color-scheme: dark)` 规则；
 *   2) `filter: invert(1) hue-rotate(180deg)`：在暗色模式下整体反相，
 *      强制旧版 llama.cpp 视觉上与软件一致。
 * 双层互补：现代页面走第 1 层（视觉无损），老页面走第 2 层（兜底）。
 */
function syncIframeTheme() {
  const wv = els.webview;
  if (!wv) return;
  const dark = !state.lightTheme;
  // 第 1 层：color-scheme 透传
  wv.style.colorScheme = dark ? 'dark' : 'light';
  // 第 2 层：暗色下整体反相
  wv.classList.toggle('match-dark', dark);
}

function toggleTheme() {
  // 立即更新状态（用于 UI 响应）
  state.lightTheme = !state.lightTheme;
  const isLight = state.lightTheme;

  // 更新按钮图标（立即响应）
  const btn = els.themeToggle;
  if (btn) {
    btn.innerHTML = isLight
      ? '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"></circle><line x1="12" y1="1" x2="12" y2="3"></line><line x1="12" y1="21" x2="12" y2="23"></line><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line><line x1="1" y1="12" x2="3" y2="12"></line><line x1="21" y1="12" x2="23" y2="12"></line><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line></svg>'
      : '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path></svg>';
  }

  // 同步 iframe 主题（立即响应）
  syncIframeTheme();

  // 使用主题引擎执行颜色动画过渡（带 300ms 防抖）
  themeManager.setLightTheme(isLight);

  showNotification(isLight ? '已切换到亮色主题' : '已切换到暗色主题', 'info', 1500);
}

// ============= 配置预设 =============
function applyConfigTemplate(templateId) {
  if (!templateId) return;
  const templates = {
    cpu_only: {
      mode: 'advanced',
      label: '纯 CPU 运行',
      params: { ngl: '-1', t: '4', tb: '4', fa: 'off', ml: '2048' },
      desc: '禁用 GPU 卸载(-ngl -1)、使用 4 线程、禁用 Flash Attention'
    },
    gpu_balanced: {
      mode: 'advanced',
      label: 'GPU 均衡（4GB+ 显存）',
      params: { ngl: '24', fa: 'on', ctk: 'q5_0', ctv: 'q5_0', t: '8', tb: '4' },
      desc: '卸载 24 层到 GPU、启用 Flash Attention、KV 缓存量化 q5_0'
    },
    gpu_full: {
      mode: 'advanced',
      label: '满速 GPU（12GB+ 显存）',
      params: { ngl: '99', fa: 'on', ctk: 'f16', ctv: 'f16', t: '16', tb: '8' },
      desc: '尽可能多卸载 GPU(-ngl 99)、F16 KV 缓存、16 线程'
    },
    low_latency: {
      mode: 'advanced',
      label: '低延迟推理',
      params: { ngl: '99', fa: 'on', n: '512', t: '16', tb: '16', ub: '128', b: '512', prio: '1' },
      desc: '小批次 512、高优先级、大批处理线程'
    },
    large_ctx: {
      mode: 'advanced',
      label: '超长上下文（32K+）',
      params: { c: '32768', ngl: '99', fa: 'on', ctk: 'q4_0', ctv: 'q4_0', ml: '8192', swa_full: 'on' },
      desc: '32K 上下文、KV 缓存量化 q4_0、8GB 缓存、SWA'
    },
    server_deploy: {
      mode: 'pro',
      label: '服务端部署',
      params: {
        customCommand: '"%%llama_server%%" --models-dir "%%models_dir%%" --host 0.0.0.0 --port %%port%% -ngl 99 -c 8192 -fa on -ctk q5_0 -ctv q5_0 --spec-type draft-mtp --spec-draft-n-max 3 -tb 8'
      },
      desc: '监听 0.0.0.0（局域网可访问）、8K 上下文、KV 缓存量化 q5_0、MTP 推测解码'
    }
  };
  const tmpl = templates[templateId];
  if (!tmpl) return;
  // 切换到对应模式
  const tab = document.querySelector(`.mode-tab[data-mode="${tmpl.mode}"]`);
  if (tab) tab.click();
  showNotification(`已应用「${tmpl.label}」预设：${tmpl.desc}`, 'info', 4000);
}

function updateWebviewVisibility() {
  const displayPort = state.activePort || state.port;
  const url = `http://127.0.0.1:${displayPort}`;
  if (els.loadingUrl) els.loadingUrl.textContent = url;

  const isRunning = state.status === 'Running';
  const hasPort = !!state.activePort;
  const wrapper = document.querySelector('.webview-wrapper');
  if (isRunning && hasPort) {
    // 运行中：显示 webview 区域
    if (wrapper) wrapper.style.display = '';
    // 仅当 iframe 还在加载时显示遮罩
    if (els.webview?.dataset.loaded !== '1') {
      showWebviewLoading();
    }
    // 用 URL 对象规范化后比较，避免路径末尾 /、大小写、片段差异造成的误判重置。
    if (els.webview) {
      const current = tryParseUrl(els.webview.src);
      const target = tryParseUrl(url);
      if (!current || !target || normalizeUrl(current) !== normalizeUrl(target)) {
        try { els.webview.src = url; } catch (e) { /* 跨域重置失败时忽略 */ }
      }
    }
    if (els.webview) els.webview.style.display = 'block';
    if (els.webviewPlaceholder) els.webviewPlaceholder.style.display = 'none';
  } else {
    hideWebviewLoading();
    if (els.webview) els.webview.style.display = 'none';
    if (els.webviewPlaceholder) els.webviewPlaceholder.style.display = 'none';
    // 未启动时隐藏整个 webview 区域
    if (wrapper) wrapper.style.display = 'none';
    if (els.webview) {
      els.webview.src = 'about:blank';
      delete els.webview.dataset.loaded;
    }
  }
}

function showWebviewLoading() {
  if (!els.webviewLoading) return;
  els.webviewLoading.hidden = false;
}
function hideWebviewLoading() {
  if (!els.webviewLoading) return;
  els.webviewLoading.hidden = true;
}

// iframe 加载监听：加载完成/失败都隐藏遮罩，避免白屏永久停留
function attachWebviewLoaders() {
  if (!els.webview) return;
  // 加载开始 → 显示遮罩
  els.webview.addEventListener('loadstart', () => {
    if (els.webview) delete els.webview.dataset.loaded;
    if (state.status === 'Running' && state.activePort) showWebviewLoading();
  });
  // 加载完成 → 标记已加载，隐藏遮罩
  els.webview.addEventListener('load', () => {
    if (els.webview) els.webview.dataset.loaded = '1';
    hideWebviewLoading();
  });
  // 加载错误 → 不标记 loaded，仍显示遮罩（用户可点"重新加载"或"停止服务"）
  els.webview.addEventListener('error', () => {
    if (state.status === 'Running' && state.activePort) showWebviewLoading();
  });
  // 超时保险：即使 load 事件不触发，30s 后也强制隐藏遮罩（页面已经显示）
  els.webview.addEventListener('load', () => {
    setTimeout(hideWebviewLoading, 0);
  });
}

// 紧急恢复：把 webview 区域重置到「未启动」状态，
// 关闭所有遮罩、重新显示 placeholder、设置 webview src 为 about:blank。
// 即使用户卡在白屏 / 加载不出，按 Ctrl+Shift+R 也能恢复。
function forceResetWebview() {
  try {
    if (els.webview) {
      delete els.webview.dataset.loaded;
      els.webview.src = 'about:blank';
      els.webview.style.display = 'none';
    }
    hideWebviewLoading();
    if (els.webviewPlaceholder) {
      els.webviewPlaceholder.style.display = 'flex';
    }
    state.activePort = null;
  } catch (e) {
    console.error('forceResetWebview:', e);
  }
}

function updatePlaceholderUrl() {
  const port = parseInt(els.port.value, 10) || DEFAULT_PORT;
  state.port = port;
  els.placeholderUrl.textContent = `http://127.0.0.1:${port}`;
}

// ============= 模式切换 =============
function setMode(mode, opts = {}) {
  if (mode !== 'normal' && mode !== 'advanced' && mode !== 'pro') mode = 'normal';
  state.mode = mode;
  els.modeTabs.forEach((tab) => tab.classList.toggle('active', tab.dataset.mode === mode));
  els.modeViews.forEach((view) => { view.hidden = view.dataset.view !== mode; });
  try {
    if (mode === 'advanced' && !opts.skipRender) {
      if (!els.advancedAccordion?.dataset.rendered) renderAdvancedAccordion();
      updateAdvancedPreview();
    } else if (mode === 'pro') {
      renderProVars();
      // 切到 pro 模式时做一次实时校验（命令可能已被加载或刚改）
      validateProCommandLive();
    } else {
      updateNormalPreview();
    }
  } catch (e) {
    console.error('setMode render failed:', e);
    appendLog({ timestamp: now(), stream: 'system', text: `[模式渲染失败] ${e?.message || e}` });
  }
  if (!opts.skipRender) scheduleSave();
}

function refreshAllPreviews() {
  updateNormalPreview();
  if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
}

function updateNormalPreview() {
  if (!els.normalCmdPreview) return;
  const program = (els.llamaServerPath.value || '').trim() || DEFAULT_PROGRAM;
  const modelsDir = els.modelsDir.value.trim() || '<未设置>';
  const port = parseInt(els.port.value, 10) || DEFAULT_PORT;
  els.normalCmdPreview.textContent =
    `${program} --models-dir "${modelsDir}" --port ${port} -ngl 99 --host 127.0.0.1`;
}

// ============= 高级模式：参数定义与渲染 =============
const ADVANCED_PARAM_GROUPS = [
  { name: '模型加载', params: [
    { key: 'm', flag: '-m', label: '模型文件', type: 'text', placeholder: 'model.gguf', flagStyle: 'short' },
    { key: 'model_url', flag: '--model-url', label: '模型 URL', type: 'text', placeholder: 'https://...' },
    { key: 'hf', flag: '-hf', label: 'HF 仓库', type: 'text', placeholder: 'user/repo[:quant]', flagStyle: 'short' },
    { key: 'hff', flag: '--hf-file', label: 'HF 文件名', type: 'text', placeholder: 'model-Q4_K_M.gguf' },
    { key: 'models_dir', flag: '--models-dir', label: '模型目录', type: 'text', placeholder: 'D:\\models', fullWidth: true },
    { key: 'models_max', flag: '--models-max', label: '最大驻留模型数', type: 'number', placeholder: '4' },
    { key: 'no_models_autoload', flag: '--no-models-autoload', label: '禁止自动扫描', type: 'checkbox' },
    { key: 'mmproj', flag: '--mmproj', label: '多模态投影', type: 'text', placeholder: 'mmproj-F16.gguf' },
  ]},
  { name: '上下文与生成', params: [
    { key: 'c', flag: '-c', label: '上下文长度', type: 'number', placeholder: '8192', flagStyle: 'short' },
    { key: 'n', flag: '-n', label: '预测 token 数', type: 'number', placeholder: '-1=无限', flagStyle: 'short' },
    { key: 'b', flag: '-b', label: '批处理大小', type: 'number', placeholder: '2048', flagStyle: 'short' },
    { key: 'ub', flag: '-ub', label: '物理批处理大小', type: 'number', placeholder: '512', flagStyle: 'short' },
    { key: 'keep', flag: '--keep', label: '保留 token', type: 'number', placeholder: '0' },
    { key: 'swa_full', flag: '--swa-full', label: '完整 SWA 缓存', type: 'checkbox' },
    { key: 'fa', flag: '-fa', label: 'Flash Attention', type: 'select', options: ['on', 'off', 'auto'], flagStyle: 'short' },
    { key: 'perf', flag: '--perf', label: '内部性能计时', type: 'checkbox' },
  ]},
  { name: '线程与 CPU', params: [
    { key: 't', flag: '-t', label: '生成线程数', type: 'number', placeholder: '8', flagStyle: 'short' },
    { key: 'tb', flag: '-tb', label: '批处理线程数', type: 'number', placeholder: '8', flagStyle: 'short' },
    { key: 'C', flag: '-C', label: 'CPU 亲和性掩码', type: 'text', placeholder: '0xf' },
    { key: 'Cr', flag: '-Cr', label: 'CPU 亲和性范围', type: 'text', placeholder: '0-7' },
    { key: 'cpu_strict', flag: '--cpu-strict', label: '严格 CPU 放置', type: 'select', options: ['0', '1'] },
    { key: 'Cb', flag: '-Cb', label: '批处理 CPU 掩码', type: 'text', placeholder: '0xf', flagStyle: 'short' },
    { key: 'Crb', flag: '-Crb', label: '批处理 CPU 范围', type: 'text', placeholder: '0-7', flagStyle: 'short' },
    { key: 'prio', flag: '--prio', label: '进程优先级', type: 'select', options: ['-1', '0', '1', '2', '3'] },
    { key: 'prio_batch', flag: '--prio-batch', label: '批处理优先级', type: 'select', options: ['-1', '0', '1', '2', '3'] },
    { key: 'poll', flag: '--poll', label: '轮询级别', type: 'number', placeholder: '50' },
  ]},
  { name: 'GPU 显存', params: [
    { key: 'ngl', flag: '-ngl', label: 'GPU 卸载层数', type: 'number', placeholder: '99', flagStyle: 'short' },
    { key: 'device', flag: '--device', label: '使用设备', type: 'text', placeholder: 'CUDA0,CUDA1' },
    { key: 'split_mode', flag: '--split-mode', label: '分割模式', type: 'select', options: ['none', 'layer', 'row', 'tensor'] },
    { key: 'ts', flag: '-ts', label: 'Tensor 分配', type: 'text', placeholder: '0.5,0.5', flagStyle: 'short' },
    { key: 'mg', flag: '-mg', label: '主 GPU 索引', type: 'number', placeholder: '0', flagStyle: 'short' },
    { key: 'fit', flag: '--fit', label: '自动适配显存', type: 'select', options: ['on', 'off'] },
    { key: 'fit_target', flag: '--fit-target', label: '显存余量(MiB)', type: 'text', placeholder: '1024' },
    { key: 'fit_ctx', flag: '--fit-ctx', label: 'fit 最小 ctx', type: 'number', placeholder: '4096' },
    { key: 'check_tensors', flag: '--check-tensors', label: '检查 tensor 有效性', type: 'checkbox' },
    { key: 'no_op_offload', flag: '--no-op-offload', label: '禁用 op 卸载', type: 'checkbox' },
  ]},
  { name: 'KV 缓存', params: [
    { key: 'nkvo', flag: '-nkvo', label: '禁用 KV 缓存卸载', type: 'checkbox', flagStyle: 'short' },
    { key: 'ctk', flag: '-ctk', label: 'K 缓存类型', type: 'select',
      options: ['f32', 'f16', 'bf16', 'q8_0', 'q4_0', 'q4_1', 'iq4_nl', 'q5_0', 'q5_1'], flagStyle: 'short' },
    { key: 'ctv', flag: '-ctv', label: 'V 缓存类型', type: 'select',
      options: ['f32', 'f16', 'bf16', 'q8_0', 'q4_0', 'q4_1', 'iq4_nl', 'q5_0', 'q5_1'], flagStyle: 'short' },
    { key: 'no_repack', flag: '--no-repack', label: '禁用权重重打包', type: 'checkbox' },
  ]},
  { name: '内存管理', params: [
    { key: 'mlock', flag: '--mlock', label: '锁定模型到内存', type: 'checkbox' },
    { key: 'no_mmap', flag: '--no-mmap', label: '禁用 mmap', type: 'checkbox' },
    { key: 'no_direct_io', flag: '--no-direct-io', label: '禁用 DirectIO', type: 'checkbox' },
    { key: 'numa', flag: '--numa', label: 'NUMA 策略', type: 'select', options: ['distribute', 'isolate', 'numactl'] },
    { key: 'cache_ram', flag: '--cache-ram', label: '最大缓存(MiB)', type: 'number', placeholder: '8192' },
  ]},
  { name: 'RoPE / YaRN', params: [
    { key: 'rope_scaling', flag: '--rope-scaling', label: 'RoPE 缩放', type: 'select', options: ['none', 'linear', 'yarn'] },
    { key: 'rope_scale', flag: '--rope-scale', label: 'RoPE 缩放因子', type: 'number', placeholder: '1' },
    { key: 'rope_freq_base', flag: '--rope-freq-base', label: 'RoPE 基础频率', type: 'number', placeholder: '10000' },
    { key: 'rope_freq_scale', flag: '--rope-freq-scale', label: 'RoPE 频率缩放', type: 'number', placeholder: '1' },
    { key: 'yarn_orig_ctx', flag: '--yarn-orig-ctx', label: 'YaRN 原始 ctx', type: 'number', placeholder: '0' },
    { key: 'yarn_ext_factor', flag: '--yarn-ext-factor', label: 'YaRN 外推因子', type: 'number', placeholder: '-1' },
    { key: 'yarn_attn_factor', flag: '--yarn-attn-factor', label: 'YaRN 注意力因子', type: 'number', placeholder: '-1' },
    { key: 'yarn_beta_slow', flag: '--yarn-beta-slow', label: 'YaRN 高修正', type: 'number', placeholder: '-1' },
    { key: 'yarn_beta_fast', flag: '--yarn-beta-fast', label: 'YaRN 低修正', type: 'number', placeholder: '-1' },
  ]},
  { name: '采样', params: [
    { key: 'temp', flag: '--temp', label: '温度', type: 'number', placeholder: '0.8' },
    { key: 'top_k', flag: '--top-k', label: 'Top-K', type: 'number', placeholder: '40' },
    { key: 'top_p', flag: '--top-p', label: 'Top-P', type: 'number', placeholder: '0.95' },
    { key: 'min_p', flag: '--min-p', label: 'Min-P', type: 'number', placeholder: '0.05' },
    { key: 'top_nsigma', flag: '--top-n-sigma', label: 'Top-N-Sigma', type: 'number', placeholder: '-1=禁用' },
    { key: 'typical_p', flag: '--typical-p', label: '典型采样', type: 'number', placeholder: '1.0' },
    { key: 'repeat_last_n', flag: '--repeat-last-n', label: '重复惩罚窗口', type: 'number', placeholder: '64' },
    { key: 'repeat_penalty', flag: '--repeat-penalty', label: '重复惩罚', type: 'number', placeholder: '1.0' },
    { key: 'presence_penalty', flag: '--presence-penalty', label: '存在惩罚', type: 'number', placeholder: '0' },
    { key: 'frequency_penalty', flag: '--frequency-penalty', label: '频率惩罚', type: 'number', placeholder: '0' },
    { key: 'seed', flag: '-s', label: '随机种子', type: 'number', placeholder: '-1', flagStyle: 'short' },
    { key: 'samplers', flag: '--samplers', label: '采样器顺序', type: 'text', placeholder: 'top_k;top_p;min_p;temperature', fullWidth: true },
  ]},
  { name: '推测解码', params: [
    { key: 'spec_type', flag: '--spec-type', label: '推测类型', type: 'select',
      options: ['none', 'draft-simple', 'draft-mtp', 'draft-eagle3', 'draft-dflash', 'ngram-simple', 'ngram-mod', 'ngram-cache'] },
    { key: 'spec_draft_n_max', flag: '--spec-draft-n-max', label: '最大 draft 数', type: 'number', placeholder: '3' },
    { key: 'spec_draft_n_min', flag: '--spec-draft-n-min', label: '最小 draft 数', type: 'number', placeholder: '0' },
    { key: 'spec_draft_p_split', flag: '--spec-draft-p-split', label: 'draft 分割概率', type: 'number', placeholder: '0.10' },
    { key: 'spec_draft_p_min', flag: '--spec-draft-p-min', label: 'draft 最小概率', type: 'number', placeholder: '0.00' },
    { key: 'model_draft', flag: '--model-draft', label: 'draft 模型', type: 'text', placeholder: 'draft.gguf' },
  ]},
  { name: '对话与模板', params: [
    { key: 'cnv', flag: '-cnv', label: '对话模式', type: 'checkbox', flagStyle: 'short' },
    { key: 'chat_template', flag: '--chat-template', label: 'Jinja 模板', type: 'text', placeholder: '模板字符串' },
    { key: 'no_jinja', flag: '--no-jinja', label: '禁用 Jinja', type: 'checkbox' },
    { key: 'reasoning', flag: '--reasoning', label: '推理模式', type: 'select', options: ['on', 'off', 'auto'] },
    { key: 'reasoning_format', flag: '--reasoning-format', label: '推理格式', type: 'select', options: ['auto', 'none', 'deepseek', 'deepseek-legacy'] },
    { key: 'reasoning_budget', flag: '--reasoning-budget', label: '推理 token 预算', type: 'number', placeholder: '-1' },
    { key: 'sys_prompt', flag: '-sys', label: '系统提示', type: 'text', placeholder: '你是一个助手…', flagStyle: 'short' },
  ]},
  { name: '日志与调试', params: [
    { key: 'verbose', flag: '-v', label: '详细日志', type: 'checkbox', flagStyle: 'short' },
    { key: 'verbosity', flag: '-lv', label: '日志详细程度', type: 'number', placeholder: '1', flagStyle: 'short' },
    { key: 'log_disable', flag: '--log-disable', label: '禁用日志', type: 'checkbox' },
    { key: 'log_file', flag: '--log-file', label: '日志文件', type: 'text', placeholder: 'path/to/log.txt' },
    { key: 'no_log_timestamps', flag: '--no-log-timestamps', label: '禁用时间戳', type: 'checkbox' },
  ]},
  { name: '服务器', params: [
    { key: 'host', flag: '--host', label: '绑定地址', type: 'text', placeholder: '127.0.0.1' },
    { key: 'port', flag: '--port', label: '监听端口', type: 'number', placeholder: String(DEFAULT_PORT) },
    { key: 'np', flag: '-np', label: '并行序列数', type: 'number', placeholder: '1', flagStyle: 'short' },
    { key: 'embedding', flag: '--embedding', label: '启用 embedding', type: 'checkbox' },
    { key: 'pooling', flag: '--pooling', label: 'Embedding 池化', type: 'select', options: ['mean', 'cls', 'last', 'rank'] },
  ]},
];

function renderAdvancedAccordion() {
  if (!els.advancedAccordion) return;
  if (els.advancedAccordion.dataset.rendered === '1') return;
  try {
    const frag = document.createDocumentFragment();
    for (const group of ADVANCED_PARAM_GROUPS) {
      const groupEl = document.createElement('div');
      groupEl.className = 'acc-group collapsed';

      const header = document.createElement('div');
      header.className = 'acc-group-header';
      header.innerHTML = `
      <span class="acc-group-arrow">▶</span>
      <span class="acc-group-title-text"></span>
      <span class="acc-group-count">${group.params.length} 项</span>
    `;
      header.querySelector('.acc-group-title-text').textContent = group.name;
      header.addEventListener('click', () => {
        const collapsed = groupEl.classList.toggle('collapsed');
        header.querySelector('.acc-group-arrow').textContent = collapsed ? '▶' : '▼';
      });
      groupEl.appendChild(header);

      const body = document.createElement('div');
      body.className = 'acc-group-body';
      for (const p of group.params) body.appendChild(buildAdvancedField(p));
      groupEl.appendChild(body);

      frag.appendChild(groupEl);
    }
    els.advancedAccordion.replaceChildren(frag);
    els.advancedAccordion.dataset.rendered = '1';
  } catch (e) {
    console.error('renderAdvancedAccordion failed:', e);
    els.advancedAccordion.textContent = `（加载失败：${e?.message || e}）`;
  }
}

function buildAdvancedField(p) {
  const wrap = document.createElement('div');
  wrap.className = 'acc-field' + (p.fullWidth ? ' full-width' : '');
  wrap.dataset.paramKey = p.key;

  const label = document.createElement('label');
  label.className = 'acc-field-label';
  const labelText = document.createElement('span');
  labelText.textContent = p.label;
  const flag = document.createElement('span');
  flag.className = 'acc-field-flag';
  flag.textContent = p.flag;
  label.appendChild(labelText);
  label.appendChild(flag);
  wrap.appendChild(label);

  if (p.type === 'checkbox') {
    const cbLabel = document.createElement('label');
    cbLabel.className = 'checkbox-label';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.dataset.key = p.key;
    cb.dataset.type = 'checkbox';
    cb.dataset.flag = p.flag;
    cb.dataset.flagStyle = p.flagStyle || 'long';
    cb.dataset.def = p.def ?? 'false';
    cb.addEventListener('change', () => { updateAdvancedPreview(); scheduleSave(); });
    const span = document.createElement('span');
    span.textContent = p.placeholder || '启用';
    cbLabel.appendChild(cb);
    cbLabel.appendChild(span);
    wrap.appendChild(cbLabel);
  } else if (p.type === 'select') {
    const sel = document.createElement('select');
    sel.dataset.key = p.key;
    sel.dataset.type = 'select';
    sel.dataset.flag = p.flag;
    sel.dataset.flagStyle = p.flagStyle || 'long';
    sel.dataset.def = p.def ?? '';
    const blank = document.createElement('option');
    blank.value = ''; blank.textContent = '（不设置）';
    sel.appendChild(blank);
    for (const o of p.options) {
      const opt = document.createElement('option');
      opt.value = o; opt.textContent = o;
      sel.appendChild(opt);
    }
    sel.addEventListener('change', () => { updateAdvancedPreview(); scheduleSave(); });
    wrap.appendChild(sel);
  } else {
    const input = document.createElement('input');
    input.type = p.type === 'number' ? 'number' : 'text';
    input.dataset.key = p.key;
    input.dataset.type = p.type;
    input.dataset.flag = p.flag;
    input.dataset.flagStyle = p.flagStyle || 'long';
    input.dataset.def = p.def ?? '';
    if (p.placeholder) input.placeholder = p.placeholder;
    input.addEventListener('input', () => { updateAdvancedPreview(); scheduleSave(); });
    wrap.appendChild(input);
  }
  return wrap;
}

function readAdvancedArgv() {
  if (!els.advancedAccordion) return [];
  const argv = [];
  els.advancedAccordion.querySelectorAll('[data-key]').forEach((el) => {
    const flag = el.dataset.flag;
    const style = el.dataset.flagStyle || 'long';
    if (el.dataset.type === 'checkbox') {
      // 布尔型 checkbox 永远是独立 flag（如 --repack / --no-mmap），不带 "on"。
      // 之前 bug：传成 "--repack on" → llama-server: invalid argument: on
      if (el.checked) argv.push({ flag, value: null, style });
    } else if (el.dataset.type === 'select') {
      if (el.value) argv.push({ flag, value: el.value, style });
    } else {
      const v = el.value.trim();
      if (v) argv.push({ flag, value: v, style });
    }
  });
  return argv;
}

function formatAdvancedArg({ flag, value, style }) {
  if (value === null || value === undefined || value === '') return flag;
  if (style === 'equals') return `${flag}=${value}`;
  return `${flag} ${value}`;
}

function updateAdvancedPreview() {
  if (!els.advancedPreview) return;
  try {
    const argv = readAdvancedArgv();
    const text = argv.map(formatAdvancedArg).join(' \\\n  ');
    els.advancedPreview.textContent = text || '（无参数）';
  } catch (e) {
    console.error('updateAdvancedPreview failed:', e);
    els.advancedPreview.textContent = `（预览错误：${e?.message || e}）`;
  }
}

function collectAdvancedAsExtraArgs() {
  return readAdvancedArgv()
    .filter((a) => a.value !== null)
    .map(formatAdvancedArg)
    .join(' ');
}

// ============= 专业模式：变量 =============

/// 专业模式可用变量（在 %%% 之间使用）。启动时被替换成实际值。
const PRO_VARS = [
  { name: 'llama_server', desc: 'llama-server 可执行文件路径' },
  { name: 'models_dir',   desc: '模型目录' },
  { name: 'port',         desc: '服务端口（普通模式下的端口）' },
  { name: 'host',         desc: '监听地址（127.0.0.1）' },
  { name: 'models_dir_quote', desc: '带引号的模型目录（"…"）' },
  { name: 'llama_server_quote', desc: '带引号的 llama-server 路径' },
];

function getProVarMap() {
  return {
    llama_server:   els.llamaServerPath?.value.trim() || '<llama-server 路径>',
    models_dir:     els.modelsDir?.value.trim() || '<模型目录>',
    port:           String(state.port || DEFAULT_PORT),
    host:           '127.0.0.1',
    models_dir_quote:   quotePath(els.modelsDir?.value.trim() || ''),
    llama_server_quote: quotePath(els.llamaServerPath?.value.trim() || ''),
  };
}

function quotePath(p) {
  if (!p) return '""';
  return /[\s"]/.test(p) ? `"${p.replace(/"/g, '\\"')}"` : p;
}

function renderProVars() {
  if (!els.varGrid) return;
  const map = getProVarMap();
  els.varGrid.replaceChildren(
    ...PRO_VARS.map(({ name, desc }) => {
      const chip = document.createElement('div');
      chip.className = 'var-chip';
      chip.title = `${desc}\n点击变量名直接插入到输入框光标位置`;

      // 变量名（点击 → 插入到光标位置）
      const nameEl = document.createElement('div');
      nameEl.className = 'var-chip-name';
      nameEl.textContent = `%%${name}%%`;
      nameEl.addEventListener('click', (e) => {
        e.stopPropagation();
        insertProVar(`%%${name}%%`);
      });

      // 当前实际值
      const valueEl = document.createElement('div');
      valueEl.className = 'var-chip-value';
      valueEl.textContent = map[name] || '';
      valueEl.title = map[name] || '';

      // 中文意思解释（灰色小字）
      const descEl = document.createElement('div');
      descEl.className = 'var-chip-desc';
      descEl.textContent = desc;

      chip.append(nameEl, valueEl, descEl);
      return chip;
    })
  );
}

/// 把文本插入到光标位置（专业模式 textarea）。
function insertProVar(text) {
  if (!els.customCommand) return;
  const ta = els.customCommand;
  const start = ta.selectionStart ?? ta.value.length;
  const end = ta.selectionEnd ?? ta.value.length;
  ta.value = ta.value.slice(0, start) + text + ta.value.slice(end);
  ta.focus();
  ta.selectionStart = ta.selectionEnd = start + text.length;
  validateProCommandLive();
  scheduleSave();
}

/** 专业模式命令实时校验：检测首 token 是否可能是 llama-server 相关。
 * 仅做"轻量"提示：拿不到后端真值时跳过。错误状态在 UI 上以红边+提示文字表示。
 * - 错误状态会被覆盖（输入即清旧错）
 * - 后端是最终防线（validate_pro_program） */
function validateProCommandLive() {
  if (!els.customCommand) return;
  const text = els.customCommand.value || '';
  // 复用 cmdline 的拆分逻辑（前端版本：与 Rust 行为一致：双引号包围、不处理反斜杠转义）
  const split = (s) => {
    const out = [];
    let cur = '';
    let inQ = false;
    for (const c of s) {
      if (c === '"') inQ = !inQ;
      else if ((c === ' ' || c === '\t') && !inQ) {
        if (cur) { out.push(cur); cur = ''; }
      } else cur += c;
    }
    if (cur) out.push(cur);
    return out;
  };
  const tokens = split(text.trim());
  if (tokens.length === 0) {
    // 空命令 → 启动时回退到普通模式，不报错
    els.customCommand.classList.remove('input-error');
    els.customCommand.title = '';
    return;
  }
  const first = tokens[0].replace(/^"+|"+$/g, '').toLowerCase();
  const stem = first.replace(/\.exe$/, '');
  // 文件名包含 llama/llamacpp → 允许
  // 纯 "llama-server" → 允许（依赖 PATH）
  // 用户配置的自定义路径 → 需在 tokens[0] 中匹配（这里做粗略检查：是否包含 llama 字样）
  const looksLikeLlama = stem.includes('llama') || stem.includes('llamacpp') || stem === 'llama-server';
  if (!looksLikeLlama) {
    els.customCommand.classList.add('input-error');
    els.customCommand.title = `首 token 不是 llama-server 相关（当前：${tokens[0]}）。启动将被后端拒绝。`;
  } else {
    els.customCommand.classList.remove('input-error');
    els.customCommand.title = '';
  }
}

// ============= 监控 =============
/** 安全地设置元素的 textContent：元素不存在时静默忽略，避免打断调用流程 */
function setText(el, text) {
  if (el) el.textContent = text;
}
/** 安全地调用 setMeter：bar 不存在时静默忽略 */
function safeSetMeter(bar, pct, opts) {
  if (bar) setMeter(bar, pct, opts);
}

function resetMetrics() {
  setText(els.metricPid, '—');
  setText(els.metricUptime, '—');
  setText(els.metricCpuText, '—');
  setText(els.metricMemVirtText, '—');
  setText(els.metricVramText, '—');
  setText(els.metricGpuText, '—');
  safeSetMeter(els.metricCpuBar, 0);
  safeSetMeter(els.metricMemVirtBar, 0);
  safeSetMeter(els.metricVramBar, 0);
  safeSetMeter(els.metricGpuBar, 0);
}

function applyMetrics(m) {
  if (!m) return;
  setText(els.metricPid, String(m.pid || '—'));
  setText(els.metricUptime, formatUptime(m.uptime_secs || 0));

  const cpuPct = Math.max(0, m.cpu_percent || 0);
  setText(els.metricCpuText, `${cpuPct.toFixed(1)}%`);
  safeSetMeter(els.metricCpuBar, cpuPct, { warn: 70, danger: 90 });

  // 虚拟地址空间：完整虚拟地址空间（含 mmap 映射的模型文件）
  // 仅用于显示加载了多大模型，不用于反映实际内存压力
  const virtBytes = m.virtual_size_bytes || 0;
  const totalMem = m.total_mem_bytes || 0;
  const virtMb = virtBytes / (1024 * 1024);
  const virtPct = totalMem > 0 ? (virtBytes / totalMem) * 100 : 0;
  setText(els.metricMemVirtText, totalMem > 0
    ? `${virtPct.toFixed(1)}% · ${virtMb.toFixed(0)} / ${(totalMem / (1024 * 1024)).toFixed(0)} MB`
    : `${virtMb.toFixed(0)} MB`);
  safeSetMeter(els.metricMemVirtBar, virtPct, { warn: 50, danger: 80 });

  const gpuUsed = m.gpu_mem_used_mb || 0;
  const gpuTotal = m.gpu_mem_total_mb || 0;
  if (gpuTotal > 0) {
    const vramPct = (gpuUsed / gpuTotal) * 100;
    setText(els.metricVramText, `${vramPct.toFixed(1)}% · ${gpuUsed.toFixed(0)} / ${gpuTotal.toFixed(0)} MB`);
    safeSetMeter(els.metricVramBar, vramPct, { warn: 70, danger: 90 });
  } else {
    setText(els.metricVramText, '不可用');
    safeSetMeter(els.metricVramBar, 0, { unavailable: true });
  }

  const gpuUtil = m.gpu_util_pct;
  if (gpuUtil == null || gpuUtil < 0) {
    setText(els.metricGpuText, '不可用');
    safeSetMeter(els.metricGpuBar, 0, { unavailable: true });
  } else {
    setText(els.metricGpuText, `${gpuUtil.toFixed(1)}%`);
    safeSetMeter(els.metricGpuBar, gpuUtil, { warn: 70, danger: 90 });
  }

  if (m.port && m.port !== state.activePort) {
    state.activePort = m.port;
    updateWebviewVisibility();
  }
}

// ============= 日志分组 =============
/** 批量 appendLog 队列：把高频日志合并到 rAF 一次性写入 DOM，避免卡顿 */
const logQueue = [];
let logFlushScheduled = false;
const LOG_BATCH_MAX = 500; // 单次 flush 最多处理行数（提升性能）
const LOG_QUEUE_MAX = 3000; // 队列上限：极端情况下丢弃最旧日志（降低内存占用）

function flushLogQueue() {
  logFlushScheduled = false;
  if (logQueue.length === 0) return;
  // 一次性取出所有待处理日志，按 group 分类
  const groups = new Map(); // groupId -> { group: Group, frag: DocumentFragment }
  let processed = 0;
  while (logQueue.length > 0 && processed < LOG_BATCH_MAX) {
    const line = logQueue.shift();
    processed++;
    const groupId = line.group || PLAIN_GROUP_ID;
    let entry = groups.get(groupId);
    if (!entry) {
      let g = state.groups[groupId];
      if (!g) g = ensureGroup(PLAIN_GROUP_ID, '常规日志', 'pending', true);
      entry = { group: g, frag: document.createDocumentFragment() };
      groups.set(groupId, entry);
    }
    const div = document.createElement('div');
    div.className = `log-line stream-${line.stream || 'stdout'}`;
    // 时间戳换行显示，防止内容挤压
    const ts = document.createElement('span');
    ts.className = 'log-time';
    ts.textContent = line.timestamp || '';
    const txt = document.createElement('span');
    txt.className = 'log-text';
    txt.textContent = line.text || '';
    div.appendChild(ts);
    div.appendChild(txt);
    entry.frag.appendChild(div);
  }
  // 一次性插入 + 截断
  for (const { group, frag } of groups.values()) {
    group.contentEl.insertBefore(frag, group.contentEl.firstChild);
    // 截断：超过上限时移除最底部的最旧行
    let extra = group.contentEl.childElementCount - MAX_LOG_LINES_PER_GROUP;
    while (extra-- > 0 && group.contentEl.lastChild) {
      group.contentEl.lastChild.remove();
    }
    if (els.autoScroll?.checked) group.contentEl.scrollTop = 0;
  }
  // 如果队列还有剩余（极端情况），继续下一帧
  if (logQueue.length > 0) {
    scheduleLogFlush();
  }
}

function scheduleLogFlush() {
  if (logFlushScheduled) return;
  logFlushScheduled = true;
  requestAnimationFrame(flushLogQueue);
}
function ensureGroup(id, name, status, autoExpand) {
  let g = state.groups[id];
  if (g) {
    g.status = status;
    g.groupEl.dataset.status = status;
    g.statusEl.textContent = STEP_STATUS_TEXT[status] || status;
    return g;
  }
  const groupEl = document.createElement('div');
  groupEl.className = 'log-group';
  groupEl.dataset.group = id;
  groupEl.dataset.status = status;

  const headerEl = document.createElement('div');
  headerEl.className = 'log-group-header';
  headerEl.innerHTML = `
    <span class="log-group-arrow">▼</span>
    <span class="log-group-title"></span>
    <span class="log-group-status"></span>
  `;
  headerEl.querySelector('.log-group-title').textContent = name || id;
  headerEl.querySelector('.log-group-status').textContent = STEP_STATUS_TEXT[status] || status;
  headerEl.addEventListener('click', () => toggleGroup(id));

  const contentEl = document.createElement('div');
  contentEl.className = 'log-group-content';

  groupEl.appendChild(headerEl);
  groupEl.appendChild(contentEl);

  // 常规日志组永远置顶（它始终承载最新活动日志）；
  // 其它组按到达顺序插到「PLAIN 之后、其它非 PLAIN 组之前」（最新组最靠上）
  if (id === PLAIN_GROUP_ID) {
    // PLAIN 永远在第一个位置
    if (els.logs.firstChild) els.logs.insertBefore(groupEl, els.logs.firstChild);
    else els.logs.appendChild(groupEl);
  } else {
    // 非 PLAIN 组：插到 PLAIN 紧邻的后面
    const plainG = state.groups[PLAIN_GROUP_ID];
    const anchor = plainG ? plainG.groupEl.nextSibling : els.logs.firstChild;
    if (anchor) els.logs.insertBefore(groupEl, anchor);
    else els.logs.appendChild(groupEl);
  }

  g = {
    id, name, status, expanded: true,
    groupEl, contentEl, headerEl,
    arrowEl: headerEl.querySelector('.log-group-arrow'),
    statusEl: headerEl.querySelector('.log-group-status'),
  };
  state.groups[id] = g;
  if (!state.groupOrder.includes(id)) {
    if (id === PLAIN_GROUP_ID) state.groupOrder.unshift(id);
    else state.groupOrder.push(id);
  }
  if (autoExpand === false) collapseGroup(id);
  else expandGroup(id);
  return g;
}

function expandGroup(id) {
  const g = state.groups[id];
  if (!g) return;
  g.expanded = true;
  g.groupEl.classList.remove('collapsed');
  g.arrowEl.textContent = '▼';
}

function collapseGroup(id) {
  const g = state.groups[id];
  if (!g) return;
  g.expanded = false;
  g.groupEl.classList.add('collapsed');
  g.arrowEl.textContent = '▶';
}

function toggleGroup(id) {
  const g = state.groups[id];
  if (!g) return;
  if (g.expanded) collapseGroup(id);
  else expandGroup(id);
}

function handleStep(step) {
  if (!step || !step.id) return;
  // 防御：缺少必要字段时静默忽略，避免后续代码崩
  const id = String(step.id);
  const name = typeof step.name === 'string' ? step.name : id;
  const status = typeof step.status === 'string' ? step.status : 'pending';
  const autoExpand = step.status === 'running' || step.auto_expand === true;
  ensureGroup(id, name, status, autoExpand);
  if (status === 'success' || status === 'failed') collapseGroup(id);
  else expandGroup(id);
}

function appendLog(line) {
  if (!line) return;
  // 队列上限：极端情况下丢弃最旧日志（防止 OOM）
  if (logQueue.length >= LOG_QUEUE_MAX) {
    const drop = logQueue.length - LOG_QUEUE_MAX + 1;
    logQueue.splice(0, drop);
  }
  logQueue.push(line);
  scheduleLogFlush();
}

function clearAllLogs() {
  els.logs.replaceChildren();
  state.groups = {};
  state.groupOrder = [];
  safeCall(() => invoke('clear_logs'));
}

// ============= 分隔条（拖拽）=============
function setupSplitter(el, side /* 'left' | 'right' */) {
  if (!el) return;
  let dragging = false;
  let startX = 0;
  let startW = 0;
  let mouseDownOnSplitter = false;

  const onMove = (e) => {
    if (!dragging || !mouseDownOnSplitter) return;
    const dx = e.clientX - startX;
    if (side === 'left') {
      const w = Math.max(MIN_PANE_W, Math.min(MAX_PANE_LEFT_W, startW + dx));
      document.documentElement.style.setProperty('--pane-left-w', `${w}px`);
    } else {
      const w = Math.max(MIN_PANE_W, Math.min(MAX_PANE_RIGHT_W, startW - dx));
      document.documentElement.style.setProperty('--pane-right-w', `${w}px`);
    }
  };
  const cleanup = () => {
    if (!dragging) return;
    dragging = false;
    mouseDownOnSplitter = false;
    el.classList.remove('active');
    document.body.style.cursor = '';
    document.removeEventListener('mousemove', onMove, true);
    document.removeEventListener('mouseup', cleanup, true);
    window.removeEventListener('blur', cleanup);
    // 恢复 iframe 交互
    if (els.webview) els.webview.style.pointerEvents = '';
  };
  el.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    dragging = true;
    mouseDownOnSplitter = true;
    startX = e.clientX;
    const cs = getComputedStyle(document.documentElement);
    startW = parseInt(side === 'left' ? cs.getPropertyValue('--pane-left-w') : cs.getPropertyValue('--pane-right-w'), 10) || 0;
    el.classList.add('active');
    document.body.style.cursor = 'col-resize';
    // 禁用 iframe 的 pointer-events，防止 WebView2 在鼠标移入时抢占事件
    if (els.webview) els.webview.style.pointerEvents = 'none';
    // capture 阶段监听，确保 iframe 上方的鼠标事件也能被父文档收到
    document.addEventListener('mousemove', onMove, true);
    document.addEventListener('mouseup', cleanup, true);
    // 兜底：应用失焦时结束拖拽（切到其他窗口）
    window.addEventListener('blur', cleanup);
    e.preventDefault();
    e.stopPropagation();
  });
}

// ============= 事件绑定 =============
function attachUIListeners() {
  // 路径输入
  [els.llamaServerPath, els.modelsDir, els.port].forEach((el) => {
    el?.addEventListener('input', () => {
      if (el === els.port) updatePlaceholderUrl();
      updateNormalPreview();
      scheduleSave();
    });
  });
  // 端口输入：实时校验，变化时刷新 placeholder URL + 变量网格 + 高级预览
  els.port?.addEventListener('input', () => {
    syncPortInput();
    updatePlaceholderUrl();
    if (state.mode === 'pro') renderProVars();
    scheduleSave();
  });
  els.port?.addEventListener('blur', () => {
    // 失焦时若校验失败，还原为 DEFAULT_PORT（避免脏值留到下次启动）
    const r = syncPortInput();
    if (!r.ok) {
      els.port.value = String(DEFAULT_PORT);
      els.port.classList.remove('input-error');
      showNotification('端口无效，已恢复为默认 10897', 'warning', 2000);
      updatePlaceholderUrl();
      if (state.mode === 'pro') renderProVars();
      scheduleSave();
    }
  });
  els.llamaServerPath?.addEventListener('input', () => {
    updateNormalPreview();
    if (state.mode === 'pro') renderProVars();
    if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
    scheduleSave();
  });
  els.modelsDir?.addEventListener('input', () => {
    updateNormalPreview();
    if (state.mode === 'pro') renderProVars();
    if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
    scheduleSave();
  });
  els.autoPort?.addEventListener('change', scheduleSave);

  // 浏览 / 检测
  els.browseLlamaServer?.addEventListener('click', () => browseFile());
  els.detectLlamaServer?.addEventListener('click', () => detectLlamaServer());
  els.browseModelsDir?.addEventListener('click', () => browseFolder());
  els.detectModelsDir?.addEventListener('click', () => detectModelsDir());

  // 模式切换（事件委托，带防抖锁）
  let modeSwitchPending = false;
  document.querySelector('.mode-tabs')?.addEventListener('click', async (e) => {
    const tab = e.target.closest('.mode-tab');
    if (!tab?.dataset.mode) return;
    const newMode = tab.dataset.mode;
    if (newMode === state.mode) return;
    if (modeSwitchPending) {
      showNotification('模式切换进行中，请稍候…', 'info', 1500);
      return;
    }
    if (state.saving) {
      // 等待保存完成，避免与新模式保存竞争
      await new Promise((r) => setTimeout(r, 60));
    }
    modeSwitchPending = true;
    try {
      // 若服务在运行，弹确认：切换模式前先停止
      if (state.status === 'Running' || state.status === 'Starting') {
        const ok = await showConfirm({
          title: '切换模式',
          body:
            '切换模式将先停止当前正在运行的 llama-server，避免端口/资源残留。\n\n' +
            '切换后需在「' +
            (newMode === 'normal' ? '普通' : newMode === 'advanced' ? '高级' : '专业') +
            '模式」中重新点「启动」。\n\n是否继续？',
          confirmText: '停止并切换',
          cancelText: '取消',
        });
        if (!ok) return;
        try {
          await invoke('stop_server');
          await waitForStatus(['Stopped', 'Crashed'], 4000);
        } catch (err) {
          appendLog({ timestamp: now(), stream: 'system', text: `停止失败：${err}` });
          showNotification(`停止失败：${err}`, 'error');
          return;
        }
      }
      setMode(newMode);
      showNotification(
        `已切换到「${newMode === 'normal' ? '普通' : newMode === 'advanced' ? '高级' : '专业'}模式」`,
        'success',
        2000
      );
    } finally {
      modeSwitchPending = false;
    }
  });

  // 高级模式
  els.advancedExpandAll?.addEventListener('click', () => toggleAllAccGroups(false));
  els.advancedCollapseAll?.addEventListener('click', () => toggleAllAccGroups(true));
  els.advancedReset?.addEventListener('click', async () => {
    const ok = await showConfirm({
      title: '重置高级设置',
      body: '将把所有参数恢复为默认值（不会影响 llama-server 路径 / 模型目录 / 端口）。\n\n是否继续？',
      confirmText: '重置',
      cancelText: '取消',
    });
    if (!ok) return;
    resetAdvancedParams();
    showNotification('已重置高级设置为默认值', 'success', 1500);
  });
  els.customCommand?.addEventListener('input', () => {
    validateProCommandLive();
    scheduleSave();
  });

  // 控制按钮（带 inFlight 防重入锁）
  const startInFlight = { value: false };
  const stopInFlight = { value: false };
  const restartInFlight = { value: false };

  els.startBtn?.addEventListener('click', async () => {
    if (startInFlight.value) return;
    if (state.status === 'Running' || state.status === 'Starting') {
      showNotification('服务已经在启动或运行中', 'info', 1500);
      return;
    }
    startInFlight.value = true;
    els.startBtn.disabled = true;
    try {
      // 先同步校验端口（避免后端拒绝时用户不知道为什么）
      const pc = syncPortInput();
      if (!pc.ok) {
        showNotification('端口无效，无法启动', 'error', 2000);
        els.port?.focus();
        return;
      }
      await invoke('save_config', { config: readConfigFromUI() });
      await invoke('start_server');
    } catch (e) {
      appendLog({ timestamp: now(), stream: 'system', text: `启动失败：${e}` });
      showNotification(`启动失败：${e}`, 'error', 4000);
    } finally {
      startInFlight.value = false;
      // 状态可能已变（Running），由 updateStatusUI 接管按钮 disabled
      if (state.status === 'Stopped' || state.status === 'Crashed') {
        els.startBtn.disabled = false;
      }
    }
  });
  els.stopBtn?.addEventListener('click', async () => {
    if (stopInFlight.value) return;
    if (state.status === 'Stopped') {
      showNotification('服务已停止，无需重复操作', 'info', 1500);
      return;
    }
    stopInFlight.value = true;
    els.stopBtn.disabled = true;
    try {
      await invoke('stop_server');
    } catch (e) {
      appendLog({ timestamp: now(), stream: 'system', text: `停止失败：${e}` });
      showNotification(`停止失败：${e}`, 'error', 4000);
    } finally {
      stopInFlight.value = false;
      if (state.status === 'Running' || state.status === 'Starting') {
        els.stopBtn.disabled = false;
      }
    }
  });
  els.restartBtn?.addEventListener('click', async () => {
    if (restartInFlight.value) return;
    if (state.status !== 'Running') {
      showNotification('服务未运行，无法重启', 'info', 1500);
      return;
    }
    restartInFlight.value = true;
    els.restartBtn.disabled = true;
    try {
      const pc = syncPortInput();
      if (!pc.ok) {
        showNotification('端口无效，无法重启', 'error', 2000);
        els.port?.focus();
        return;
      }
      await invoke('save_config', { config: readConfigFromUI() });
      await invoke('restart_server');
    } catch (e) {
      appendLog({ timestamp: now(), stream: 'system', text: `重启失败：${e}` });
      showNotification(`重启失败：${e}`, 'error', 4000);
    } finally {
      restartInFlight.value = false;
      if (state.status === 'Running') {
        els.restartBtn.disabled = false;
      }
    }
  });
  els.clearLogs?.addEventListener('click', clearAllLogs);
  els.exportLogs?.addEventListener('click', handleExportLogs);
  els.exportConfig?.addEventListener('click', handleExportConfig);
  els.importConfig?.addEventListener('click', handleImportConfig);
  els.autoScroll?.addEventListener('change', scheduleSave);

  // WebView 加载遮罩操作按钮
  const handleOpenInBrowser = async () => {
    const url = `http://127.0.0.1:${state.activePort || state.port}`;
    try {
      await invoke('open_external_url', { url });
      showNotification('已在浏览器中打开', 'success', 1500);
    } catch (e) {
      appendLog({ timestamp: now(), stream: 'system', text: `无法打开浏览器：${e}` });
      showNotification('打开浏览器失败，请检查是否已安装默认浏览器', 'error', 2500);
    }
  };
  els.openInBrowser?.addEventListener('click', handleOpenInBrowser);
  els.openInBrowserToolbar?.addEventListener('click', handleOpenInBrowser);
  els.reloadWebview?.addEventListener('click', () => {
    if (!els.webview) return;
    if (els.webview) delete els.webview.dataset.loaded;
    try {
      const url = `http://127.0.0.1:${state.activePort || state.port}?t=${Date.now()}`;
      els.webview.src = url;
      showWebviewLoading();
    } catch (e) {
      appendLog({ timestamp: now(), stream: 'system', text: `重新加载失败：${e}` });
    }
  });
  els.stopFromLoading?.addEventListener('click', async () => {
    if (stopInFlight.value) return;
    stopInFlight.value = true;
    try {
      await invoke('stop_server');
      showNotification('已停止服务', 'success', 1500);
    } catch (e) {
      showNotification(`停止失败：${e}`, 'error', 3000);
    } finally {
      stopInFlight.value = false;
    }
  });

  // ---- 主题切换 ----
  els.themeToggle?.addEventListener('click', toggleTheme);

  // ---- 配置预设 ----
  els.configTemplateSelect?.addEventListener('change', (e) => {
    applyConfigTemplate(e.target.value);
    e.target.value = ''; // 复位到占位项
  });

  // ---- 拖拽支持：模型目录 ----
  const dropZones = [els.modelsDir, els.llamaServerPath];
  dropZones.forEach((el) => {
    if (!el) return;
    // 阻止浏览器默认拖拽打开链接行为
    el.addEventListener('dragenter', (e) => {
      e.preventDefault();
      e.stopPropagation();
      el.classList.add('drag-over');
    });
    el.addEventListener('dragover', (e) => {
      e.preventDefault();
      e.stopPropagation();
    });
    el.addEventListener('dragleave', (e) => {
      e.preventDefault();
      e.stopPropagation();
      el.classList.remove('drag-over');
    });
    el.addEventListener('drop', (e) => {
      e.preventDefault();
      e.stopPropagation();
      el.classList.remove('drag-over');
      const files = e.dataTransfer?.files;
      if (!files?.length) return;
      const path = files[0].path || files[0].name;
      el.value = path;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      showNotification(`已填入路径：${path}`, 'success', 2000);
    });
  });
}

function toggleAllAccGroups(collapse) {
  if (!els.advancedAccordion) return;
  els.advancedAccordion.querySelectorAll('.acc-group').forEach((g) => {
    g.classList.toggle('collapsed', collapse);
    const a = g.querySelector('.acc-group-arrow');
    if (a) a.textContent = collapse ? '▶' : '▼';
  });
}

// 把所有高级设置的开关/输入框恢复为默认值（数据来自 ADVANCED_PARAM_GROUPS）
function resetAdvancedParams() {
  if (!els.advancedAccordion) return;
  els.advancedAccordion.querySelectorAll('[data-key]').forEach((el) => {
    const def = el.dataset.def;
    if (el.dataset.type === 'checkbox') {
      el.checked = def === 'true' || def === '1' || def === 'on';
    } else if (el.dataset.type === 'number' || el.dataset.type === 'text') {
      el.value = def || '';
    } else if (el.dataset.type === 'select') {
      el.value = def || el.options?.[0]?.value || '';
    }
  });
  updateAdvancedPreview();
  scheduleSave();
}

// ============= 业务动作 =============
// 浏览 / 检测按钮防重入：openDialog 是异步的，防止快速连点导致多个 dialog 叠加
let browseInFlight = { value: false };

async function browseFile() {
  if (browseInFlight.value) return;
  browseInFlight.value = true;
  try {
    const path = await openDialog({
      title: '选择 llama-server 可执行文件',
      multiple: false,
      directory: false,
      filters: [{ name: '可执行文件', extensions: ['exe', ''] }, { name: '所有文件', extensions: ['*'] }],
    });
    if (path) {
      els.llamaServerPath.value = path;
      showNotification(`已设置 llama-server 路径：${path}`, 'success');
      updateNormalPreview();
      if (state.mode === 'pro') renderProVars();
      if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
      scheduleSave();
    }
  } catch (e) {
    showNotification(`浏览失败：${e}`, 'error', 3000);
  } finally {
    browseInFlight.value = false;
  }
}

async function browseFolder() {
  if (browseInFlight.value) return;
  browseInFlight.value = true;
  try {
    const path = await openDialog({
      title: '选择包含 .gguf 模型文件的目录',
      multiple: false,
      directory: true,
    });
    if (path) {
      els.modelsDir.value = path;
      updateNormalPreview();
      if (state.mode === 'pro') renderProVars();
      if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
      // 用户放弃自动检测、手动选择 → 选择完成后做合规性检查
      try {
        const result = await invoke('check_models_dir', { path });
        if (result && result.valid) {
          showNotification(result.message || `已设置模型目录：${path}`, 'success');
        } else {
          const msg = result?.message || `该目录不包含 .gguf 模型文件：${path}`;
          showNotification(msg, 'warning', 4000);
        }
      } catch (e) {
        // 检查命令本身失败不影响保存配置；给出 warning 即可
        showNotification(`目录扫描失败：${e}`, 'warning', 3000);
      }
      scheduleSave();
    }
  } catch (e) {
    showNotification(`浏览目录失败：${e}`, 'error', 3000);
  } finally {
    browseInFlight.value = false;
  }
}

async function detectLlamaServer() {
  const result = await startInlineScanFlow('llama', {
    onResult: (res) => {
      if (res && res.found && res.path) {
        els.llamaServerPath.value = res.path;
        // 通知已合并在扫描 Toast 中，不再额外调用 showNotification
        updateNormalPreview();
        if (state.mode === 'pro') renderProVars();
        if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
        scheduleSave();
      }
      // 失败情况已由 finalizeInlineScan 在扫描 Toast 中显示，此处不再重复
    },
  });
}

async function detectModelsDir() {
  const result = await startInlineScanFlow('models', {
    onResult: (res) => {
      if (res && res.found && res.path) {
        els.modelsDir.value = res.path;
        // 通知已合并在扫描 Toast 中，不再额外调用 showNotification
        updateNormalPreview();
        if (state.mode === 'pro') renderProVars();
        if (els.advancedAccordion?.dataset.rendered) updateAdvancedPreview();
        scheduleSave();
      }
      // 失败情况已由 finalizeInlineScan 在扫描 Toast 中显示，此处不再重复
    },
  });
}

// ============================================================
// 自动检测：合并到顶部通知（Toast 内嵌进度条）
// 不再使用内联状态条，进度直接显示在顶部通知中。
// ============================================================

/** 当前正在进行的检测 kind → toast 引用 */
const _scanToast = { llama: null, models: null };
/** 当前正在进行的检测 kind（用于事件过滤）；null = 空闲 */
let _inlineScanningKind = null;

/** 创建或替换一个扫描进度 Toast */
function createScanToast(kind, message) {
  // 关闭已有同类型 toast
  if (_scanToast[kind]) {
    const old = _scanToast[kind];
    old.toast.remove();
    _scanToast[kind] = null;
  }

  const toast = document.createElement('div');
  // P-fix-v8: 加 .with-progress 类承载布局，与状态类（scanning/success/error/warning）解耦
  // finalizeInlineScan 会移除 .scanning，但 .with-progress 永不移除，保证布局不丢
  toast.className = 'toast with-progress scanning';
  toast.innerHTML = `
    <div class="toast-row">
      <span class="toast-icon">◐</span>
      <span class="toast-text"></span>
      <span class="toast-elapsed">0.0s</span>
      <button class="toast-close" type="button" aria-label="关闭">×</button>
    </div>
    <div class="toast-progress-bar"><div class="toast-progress-fill" style="width: 5%"></div></div>
  `;
  toast.querySelector('.toast-text').textContent = message;
  els.toastContainer.appendChild(toast);
  requestAnimationFrame(() => toast.classList.add('show'));

  const entry = {
    toast,
    text: toast.querySelector('.toast-text'),
    elapsed: toast.querySelector('.toast-elapsed'),
    bar: toast.querySelector('.toast-progress-fill'),
    startedAt: performance.now(),
  };

  // 关闭按钮仅隐藏 toast，不取消扫描
  toast.querySelector('.toast-close').addEventListener('click', () => {
    toast.classList.remove('show');
    setTimeout(() => { toast.remove(); if (_scanToast[kind] === entry) _scanToast[kind] = null; }, 220);
  });

  _scanToast[kind] = entry;
  return entry;
}

/** 打开/重置扫描进度到 Toast */
function startInlineScan(kind) {
  const entry = createScanToast(kind, kind === 'llama'
    ? '正在检测 llama-server：① 环境变量…'
    : '正在检测模型目录：① 环境变量…');
  _inlineScanningKind = kind;
  return entry;
}

/** 由 detect-progress 事件回调，更新 Toast 内容 */
function updateInlineScan(kind, p) {
  if (p.kind !== kind) return;
  const entry = _scanToast[kind];
  if (!entry) return;

  const stageToPercent = { 1: 25, 2: 50, 3: 75, 4: 100 };
  if (p.stage && stageToPercent[p.stage] && p.status === 'running') {
    entry.bar.style.width = stageToPercent[p.stage] + '%';
  }

  const stageNames = {
    llama: ['① 环境变量 / PATH', '② 虚拟环境扫描', '③ 关键目录匹配', '④ 全盘深度扫描（兜底）'],
    models: ['① 环境变量 / 配置', '② 关联目录匹配', '③ 关联 llama-server', '④ 全盘深度扫描（兜底）'],
  };
  const idx = Math.max(0, Math.min(3, (p.stage || 1) - 1));
  const stageName = (stageNames[kind] || [])[idx] || '';

  if (p.status === 'running') {
    entry.text.textContent = `${stageName}：${p.message || '检查中…'}`;
  } else if (p.status === 'found') {
    entry.text.textContent = `✓ ${stageName}：${p.message || '已命中'}`;
  } else if (p.status === 'cancelled') {
    entry.text.textContent = `已取消：${p.message || ''}`;
  } else if (p.status === 'timeout') {
    entry.text.textContent = `超时：${p.message || ''}`;
  }

  if (p.elapsed_ms != null) {
    entry.elapsed.textContent = `${(p.elapsed_ms / 1000).toFixed(1)}s`;
  }
}

/** 收尾：扫描完成 → 等待 3 秒自动消失 */
function finalizeInlineScan(kind, status, result) {
  const entry = _scanToast[kind];
  if (!entry) return;

  _inlineScanningKind = null;

  // 切换状态样式
  entry.toast.classList.remove('scanning');
  if (status === 'success') {
    entry.toast.classList.add('success');
    if (result && result.path) {
      // 显示与原 showNotification 一致的文字，进度条在其下方
      const prefix = kind === 'llama' ? '已检测到 llama-server'
        : '已自动识别模型目录';
      entry.text.textContent = `${prefix}：${result.path}`;
    }
    // 进度条平滑走到 100%
    entry.bar.style.transition = 'width 1s cubic-bezier(0.16, 1, 0.3, 1)';
    requestAnimationFrame(() => {
      requestAnimationFrame(() => { entry.bar.style.width = '100%'; });
    });
  } else if (status === 'cancelled') {
    entry.toast.classList.add('warning');
    entry.text.textContent = `已取消：${result?.message || ''}`;
  } else {
    entry.toast.classList.add('error');
    entry.text.textContent = `✕ ${result?.message || '未找到'}`;
  }

  // 等待 3 秒后自动消失
  setTimeout(() => {
    if (entry.toast.isConnected) {
      entry.toast.classList.remove('show');
      setTimeout(() => {
        entry.toast.remove();
        if (_scanToast[kind] === entry) _scanToast[kind] = null;
      }, 220);
    }
  }, 3000);
}

/** 全局 detect-progress 事件 handler */
function onDetectProgressInline(p) {
  if (!p) return;
  if (_inlineScanningKind) {
    updateInlineScan(_inlineScanningKind, p);
  }
}

/**
 * 启动一次扫描。返回 Promise<DetectResult>。
 * 自动：
 *  - 创建进度 Toast 在最顶部
 *  - 监听 progress 事件更新 Toast 内容
 *  - 完成后切换 success/failure 样式，3 秒后自动消失
 *  - 防重入：已有扫描时拒绝并提示
 */
async function startInlineScanFlow(kind, opts = {}) {
  if (_inlineScanningKind) {
    showNotification(`已有 ${_inlineScanningKind === 'llama' ? 'llama-server' : '模型目录'} 检测在进行，请稍候`, 'info', 2000);
    return null;
  }

  // 创建进度 Toast
  const ctx = startInlineScan(kind);

  // 本地计时（progress 事件不会每帧都发，本地补上）
  if (window.__inlineScanTimer) clearInterval(window.__inlineScanTimer);
  window.__inlineScanTimer = setInterval(() => {
    if (!_inlineScanningKind && !_scanToast[kind]) {
      clearInterval(window.__inlineScanTimer);
      window.__inlineScanTimer = null;
      return;
    }
    const entry = _scanToast[kind];
    if (!entry || !entry.toast.isConnected || !entry.toast.classList.contains('scanning')) return;
    const s = (performance.now() - entry.startedAt) / 1000;
    entry.elapsed.textContent = `${s.toFixed(1)}s`;
  }, 100);

  try {
    const cmd = kind === 'llama' ? 'detect_llama_server' : 'detect_models_dir';
    const result = await invoke(cmd);
    if (window.__inlineScanTimer) {
      clearInterval(window.__inlineScanTimer);
      window.__inlineScanTimer = null;
    }
    if (result && result.found && result.path) {
      finalizeInlineScan(kind, 'success', result);
    } else {
      finalizeInlineScan(kind, 'failure', { message: result?.message || '在所有阶段均未找到' });
    }
    if (opts.onResult) opts.onResult(result);
    return result;
  } catch (e) {
    if (window.__inlineScanTimer) {
      clearInterval(window.__inlineScanTimer);
      window.__inlineScanTimer = null;
    }
    finalizeInlineScan(kind, 'failure', { message: `检测失败：${e}` });
    if (opts.onResult) opts.onResult({ kind, found: false, message: String(e) });
    return null;
  }
}

// ============= 启动流程 =============
async function init() {
  // 全局错误捕获：把未捕获异常写到日志，避免白屏
  window.addEventListener('error', (ev) => {
    console.error('[uncaught]', ev.error || ev.message);
    appendLog({ timestamp: now(), stream: 'system', text: `[JS 错误] ${ev.error?.stack || ev.message || ev}` });
  });
  window.addEventListener('unhandledrejection', (ev) => {
    console.error('[unhandled rejection]', ev.reason);
    appendLog({ timestamp: now(), stream: 'system', text: `[JS Promise 错误] ${ev.reason?.stack || ev.reason}` });
  });

  attachUIListeners();
  setupSplitter(els.splitter, 'left');
  setupSplitter(els.splitterRight, 'right');
  attachWebviewLoaders();

  // 启动时按当前主题状态同步一次 iframe 配色（避免 reload 后 class/style 丢失）
  syncIframeTheme();

  // 全局键盘快捷键
  document.addEventListener('keydown', (e) => {
    // 忽略输入框/文本域内的快捷键（避免在输入内容时误触）
    const tag = e.target?.tagName;
    const isInput = tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';

    if (e.key === 'Escape') {
      // 关闭确认弹窗（如果开着）
      if (els.modal && !els.modal.hidden) {
        els.modal.hidden = true;
      }
      // 关闭所有 toast
      els.toastContainer?.querySelectorAll('.toast').forEach((t) => t.remove());
    }

    // Ctrl+Shift+R: 强制重置 WebView 区域（紧急恢复）
    if (e.ctrlKey && e.shiftKey && (e.key === 'R' || e.key === 'r')) {
      e.preventDefault();
      forceResetWebview();
      showNotification('已强制重置 WebView 区域', 'info', 1500);
      return;
    }

    // Ctrl+S: 保存配置
    if (e.ctrlKey && !e.shiftKey && (e.key === 's' || e.key === 'S')) {
      e.preventDefault();
      scheduleSave.flush?.() || scheduleSave();
      showNotification('配置已保存', 'success', 1000);
      return;
    }

    // Ctrl+R: 重启服务（仅在运行中有效）
    if (e.ctrlKey && !e.shiftKey && !isInput && (e.key === 'r' || e.key === 'R')) {
      e.preventDefault();
      els.restartBtn?.click();
      return;
    }

    // Ctrl+Space: 检测 llama-server（仅在非输入焦点时）
    if (e.ctrlKey && !isInput && e.key === ' ') {
      e.preventDefault();
      els.detectLlamaServer?.click();
      return;
    }
  });

  // iframe 加载超时保险：60s 内还没加载完，提示用户但仍保留遮罩（用户可手动重试/停止）

  // 事件订阅
  const subs = [
    listen('server-log', (e) => appendLog(e.payload)),
    listen('server-status', (e) => updateStatusUI(e.payload)),
    listen('server-metrics', (e) => applyMetrics(e.payload)),
    listen('server-step', (e) => handleStep(e.payload)),
    listen('detect-progress', (e) => onDetectProgressInline(e.payload)),
    // 关闭窗口时若 llama 仍在运行，弹出确认提示
    listen('close-requested', async () => {
      const confirmed = await showConfirm({
        title: 'Llama 仍在运行',
        body: 'Llama 服务正在运行中，关闭应用将同时停止服务。确定要关闭吗？',
        confirmText: '关闭并退出',
        cancelText: '取消',
      });
      if (confirmed) {
        try {
          await invoke('force_close');
        } catch (e) {
          appendLog({ timestamp: now(), stream: 'system', text: `关闭失败：${e}` });
        }
      }
    }),
  ];
  await Promise.all(subs);

  // 加载配置
  safeCall(async () => {
    const cfg = await invoke('load_config');
    writeConfigToUI(cfg);
    state.port = cfg.port || DEFAULT_PORT;
  }, '加载配置失败');

  // 加载状态
  safeCall(async () => {
    const status = await invoke('get_status');
    if (status.port) state.port = status.port;
    if (status.active_port) state.activePort = status.active_port;
    updateStatusUI(status.status);
  }, '获取状态失败');

  // 加载历史日志
  // 启动时只加载最近 200 行，避免构建 5000+ DOM 节点卡住 UI。
  // 后续新日志通过 rAF 批量追加，效率更高。
  safeCall(async () => {
    const logs = await invoke('get_logs');
    if (logs?.length) {
      // 先确保 PLAIN group 存在
      ensureGroup(PLAIN_GROUP_ID, '常规日志', 'pending', true);
      const plainGroup = state.groups[PLAIN_GROUP_ID];
      const initial = logs.slice(-200);
      // 初始历史日志的 group 可能是 null（已弃用分组）或 '_plain'。
      // 全部归并到 PLAIN group 的 contentEl，保证与新日志共享同一容器。
      for (const line of initial) {
        const div = buildLogFragment(line);
        plainGroup.contentEl.appendChild(div);
      }
      // 截断：超过上限时移除最底部的最旧行
      let extra = plainGroup.contentEl.childElementCount - MAX_LOG_LINES_PER_GROUP;
      while (extra-- > 0 && plainGroup.contentEl.lastChild) {
        plainGroup.contentEl.lastChild.remove();
      }
      // 滚到顶部（最新在最上）
      if (els.autoScroll?.checked) plainGroup.contentEl.scrollTop = 0;
      if (logs.length > 200) {
        appendLog({
          timestamp: now(),
          stream: 'system',
          text: `已加载最近 200 条日志（共 ${logs.length} 条）。`,
        });
      }
    }
  }, '获取日志失败');

  // 自动检测 llama-server（顺序执行，确保 config 在 run_initialization 前已保存）
  // 关键修复：必须 await + 立即 save_config（不走 debounce），
  // 否则 run_initialization 读到的是旧 config（空 models_dir → 环境检查失败）
  await autoDetectAndSave();

  // 启动后自动执行三步初始化（init() 顺序末尾执行）
  safeCall(async () => {
    await invoke('run_initialization');
  }, '初始化失败');
}

/**
 * 启动时自动检测：先 llama-server，再（同盘符/同级）models 目录。
 * 每一步命中后立即 `await save_config` 写盘，确保后续 `run_initialization`
 * 读到的是最新配置（不再有「明明有 models 但 env-check 报不存在」的问题）。
 */
async function autoDetectAndSave() {
  if (!els.llamaServerPath.value) {
    try {
      const result = await invoke('detect_llama_server');
      if (result && result.found && result.path) {
        els.llamaServerPath.value = result.path;
        // 静默填入，用户可见输入框内容变化；通知已合并在扫描 Toast 中
        try {
          await invoke('save_config', { config: readConfigFromUI() });
        } catch (e) {
          console.error('[autoDetect] save llama-server path failed:', e);
        }
      }
    } catch (e) {
      console.error('[autoDetect] detect_llama_server failed:', e);
    }
  }

  // 找到 llama-server 后，立即尝试自动定位同级 models 目录
  if (els.llamaServerPath.value && !els.modelsDir.value) {
    try {
      const mr = await invoke('detect_models_dir');
      if (mr && mr.found && mr.path) {
        els.modelsDir.value = mr.path;
        // 静默填入，通知已合并在扫描 Toast 中
        try {
          await invoke('save_config', { config: readConfigFromUI() });
        } catch (e) {
          console.error('[autoDetect] save models_dir failed:', e);
        }
      }
    } catch (e) {
      console.error('[autoDetect] detect_models_dir failed:', e);
    }
  }
}

function buildLogFragment(line) {
  const div = document.createElement('div');
  div.className = `log-line stream-${line.stream || 'stdout'}`;
  const ts = document.createElement('span');
  ts.className = 'log-time'; ts.textContent = line.timestamp || '';
  const txt = document.createElement('span');
  txt.className = 'log-text'; txt.textContent = line.text || '';
  div.appendChild(ts);
  div.appendChild(txt);
  return div;
}

// ============================================================
// 日志导出功能
// ============================================================

/**
 * 导出日志到文件。
 * 支持 txt/json/csv 三种格式。
 */
async function handleExportLogs() {
  // 简单格式选择
  const format = prompt('导出格式（txt/json/csv）：', 'txt');
  if (!format || !['txt', 'json', 'csv'].includes(format)) {
    showNotification('请选择有效的导出格式（txt、json 或 csv）', 'warning');
    return;
  }

  const { save } = window.__TAURI__.dialog;
  const path = await save({
    defaultPath: `llamaui-logs-${Date.now()}.${format}`,
    filters: [{
      name: `日志文件 (*.${format})`,
      extensions: [format]
    }]
  });

  if (!path) return; // 用户取消

  try {
    await invoke('export_logs', {
      req: {
        format: format,
        path: path,
        scope: 'all'
      }
    });
    showNotification('日志导出成功', 'success');
  } catch (e) {
    showNotification('导出失败：' + e, 'error');
  }
}

// ============================================================
// 配置导入/导出功能
// ============================================================

/**
 * 导出配置到文件
 */
async function handleExportConfig() {
  const { save } = window.__TAURI__.dialog;
  const path = await save({
    defaultPath: `llamaui-config-${new Date().toISOString().slice(0,10)}.json`,
    filters: [{ name: 'JSON 配置文件', extensions: ['json'] }]
  });
  if (!path) return;
  try {
    await invoke('export_config_to_file', { path });
    showNotification('配置已导出', 'success');
  } catch (e) {
    showNotification('导出失败：' + e, 'error');
  }
}

/**
 * 从文件导入配置
 */
async function handleImportConfig() {
  const ok = await showConfirm({ title: '导入配置', body: '导入配置将覆盖当前设置，确定继续？', confirmText: '继续', cancelText: '取消' });
  if (!ok) return;
  const { open } = window.__TAURI__.dialog;
  const path = await open({
    multiple: false,
    filters: [{ name: 'JSON 配置文件', extensions: ['json'] }]
  });
  if (!path) return;
  try {
    await invoke('import_config_from_file', { path: typeof path === 'string' ? path : path[0] });
    showNotification('配置已导入，正在刷新...', 'success');
    await loadConfigToForm();
  } catch (e) {
    showNotification('导入失败：' + e, 'error');
  }
}

// ============================================================
// 启动
// ============================================================

init().catch((e) => console.error('init error:', e));
