/**
 * 前端事件总线 + IPC 封装
 *
 * 职责：
 *  - EventBus：统一事件订阅/触发，解决旧版 67 个分散 addEventListener
 *    的问题；提供 once、throttle、自动清理（cleanupAll）。
 *  - IPC：对 Tauri invoke 做错误标准化与简易重试，错误统一携带命令名与参数。
 */

// ---------- 错误类型 ----------
/** Tauri 原生错误（invoke 抛出的 std::Error） */
export class TauriError extends Error {
  constructor(message, code, payload) {
    super(message);
    this.name = 'TauriError';
    this.code = code;
    this.payload = payload;
  }
}

/** 前端业务错误（状态机拒绝、参数非法等） */
export class AppError extends Error {
  constructor(message, code) {
    super(message);
    this.name = 'AppError';
    this.code = code;
  }
}

/** IPC 调用失败（TauriError + 命令上下文） */
export class IPCError extends Error {
  constructor(command, args, cause) {
    super(`IPC invoke failed: ${command}(${JSON.stringify(args)})`);
    this.name = 'IPCError';
    this.command = command;
    this.args = args;
    this.cause = cause;
  }
}

// ---------- 事件总线 ----------
export class EventBus {
  /** @param {number} defaultThrottleMs 全局节流默认间隔 */
  constructor(defaultThrottleMs = 300) {
    /** @type {Map<string, {fn:(payload)=>void, once:boolean}[]>} */
    this._subs = new Map();
    /** @type {Map<string, {timer:number|null}>} */
    this._throttled = new Map();
    this._defaultThrottleMs = defaultThrottleMs;
  }

  /**
   * 订阅事件
   * @template T
   * @param {string} eventName
   * @param {(payload:T)=>void} fn
   * @param {object} [options]
   * @param {boolean} [options.once] 只触发一次
   * @param {number} [options.throttle] 节流间隔（ms），覆盖全局默认
   */
  on(eventName, fn, options = {}) {
    if (!this._subs.has(eventName)) {
      this._subs.set(eventName, []);
    }
    const entries = this._subs.get(eventName);
    const once = !!options.once;
    const throttleMs =
      options.throttle !== undefined ? options.throttle : this._defaultThrottleMs;
    let wrapped = fn;
    if (throttleMs > 0) {
      let timer = null;
      let lastTriggered = 0;
      const throttled = /** @param {T} p */ (p) => {
        const now = Date.now();
        if (now - lastTriggered >= throttleMs) {
          lastTriggered = now;
          fn(p);
        } else if (!timer) {
          // 尾部触发：保证在间隔结束后至少再触发一次
          timer = /** @type {any} */ (setTimeout(() => {
            timer = null;
            lastTriggered = Date.now();
            fn(p);
          }, throttleMs - (now - lastTriggered)));
        }
      };
      wrapped = throttled;
    }
    entries.push({ fn: wrapped, once });
    return {
      off: () => {
        const arr = this._subs.get(eventName);
        if (!arr) return;
        const idx = arr.findIndex((e) => e.fn === wrapped);
        if (idx !== -1) arr.splice(idx, 1);
      },
    };
  }

  /** 只触发一次的订阅 */
  once(eventName, fn) {
    return this.on(eventName, fn, { once: true });
  }

  /** 取消某函数的订阅 */
  off(eventName, fn) {
    const arr = this._subs.get(eventName);
    if (!arr) return;
    for (let i = 0; i < arr.length; i++) {
      if (arr[i].fn === fn) {
        arr.splice(i, 1);
        return;
      }
    }
  }

  /** 触发事件 */
  emit(eventName, payload) {
    const entries = this._subs.get(eventName);
    if (!entries) return;
    // emit 后立即清空订阅：once 已消费，normal 重新订阅后才会收到下一次
    this._subs.delete(eventName);
    for (const { fn } of entries) {
      try {
        fn(payload);
      } catch (err) {
        // 单个监听器异常不应中断整批分发
        console.error(`[EventBus] listener error on "${eventName}":`, err);
      }
    }
  }

  /** 一次性清理所有订阅并重置节流器 */
  cleanupAll() {
    for (const [name, entries] of this._subs) {
      for (const entry of entries) {
        if (entry.once) continue; // once 订阅已被删除，无需清节流
      }
    }
    this._subs.clear();
    for (const [, state] of this._throttled) {
      if (state.timer !== null) clearTimeout(state.timer);
    }
    this._throttled.clear();
  }

  /** 当前是否还有订阅者 */
  hasListeners(eventName) {
    return (this._subs.get(eventName) || []).length > 0;
  }
}

// ---------- IPC 封装 ----------
/** @typedef {typeof window.__TAURI__} TauriCore */
export class IPC {
  /**
   * @param {TauriCore} [tauri] 默认使用全局 __TAURI__
   */
  constructor(tauri) {
    this._tauri = tauri || (typeof window !== 'undefined' ? window.__TAURI__ : undefined);
  }

  /** 执行一次带简易重试的 IPC 调用 */
  async invoke(command, args = {}, options = {}) {
    const { retries = 2, retryInterval = 1000 } = options;
    let lastErr = null;
    for (let attempt = 0; attempt <= retries; attempt++) {
      try {
        return await this._call(command, args);
      } catch (err) {
        lastErr = err;
        // 客户端错误（参数/状态问题）重试无意义，直接抛
        if (!(err instanceof TauriError)) break;
        if (attempt < retries) {
          await new Promise((r) => setTimeout(r, retryInterval));
        }
      }
    }
    throw lastErr || new Error('invoke failed');
  }

  /** 单次调用（不重试），供状态机内部使用 */
  async _call(command, args = {}) {
    if (!this._tauri) {
      throw new Error('Tauri core not available');
    }
    try {
      const result = await this._tauri.core.invoke(command, args);
      return result;
    } catch (err) {
      let tauriErr = err;
      if (err instanceof Error && err.name === 'TauriError') {
        tauriErr = new TauriError(
          err.message,
          err.code ?? undefined,
          err.payload ?? undefined
        );
      } else if (err instanceof Error) {
        tauriErr = new TauriError(err.message, undefined, err);
      } else {
        tauriErr = new TauriError(String(err), undefined, err);
      }
      throw new IPCError(command, args, tauriErr);
    }
  }

  /** 绑定带上下文的调用，方便 in 链式 */
  in(command) {
    return (args) => this.invoke(command, args);
  }
}

export default EventBus;