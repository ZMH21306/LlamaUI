// bus.mjs - Event bus and IPC wrapper
import { EVENTS } from './constants/events.mjs';

/** Custom error classes */
class TauriError extends Error {
  constructor(message, code, payload) {
    super(message);
    this.name = 'TauriError';
    this.code = code;
    this.payload = payload;
  }
}

class AppError extends Error {
  constructor(message, code) {
    super(message);
    this.name = 'AppError';
    this.code = code;
  }
}

class IPCError extends Error {
  constructor(command, args, cause) {
    super(`IPC invoke failed: ${command}(${JSON.stringify(args)})`);
    this.name = 'IPCError';
    this.command = command;
    this.args = args;
    this.cause = cause;
  }
}

/** EventBus: Simple pub/sub with throttling and cleanup */
class EventBus {
  constructor(defaultThrottleMs = 100) {
    /** @type {Map<string, {fn: Function, once: boolean, throttle: number}[]>} */
    this._subs = new Map();
    this._defaultThrottleMs = defaultThrottleMs;
    this._throttleTimers = new Map(); // eventName -> { last: number, timer: NodeJS.Timeout }
  }

  /**
   * Subscribe to an event
   * @param {string} eventName
   * @param {Function} fn
   * @param {Object} [options] - { once: boolean, throttle: number }
   * @returns {{ off: () => void }}
   */
  on(eventName, fn, options = {}) {
    if (!this._subs.has(eventName)) {
      this._subs.set(eventName, []);
    }
    const entries = this._subs.get(eventName);
    const { once = false, throttle = this._defaultThrottleMs } = options;
    
    let handler = fn;
    if (throttle > 0) {
      let lastCall = 0;
      let timer = null;
      handler = (...args) => {
        const now = Date.now();
        if (now - lastCall >= throttle) {
          lastCall = now;
          if (timer) clearTimeout(timer);
          timer = null;
          fn(...args);
        } else if (!timer) {
          timer = setTimeout(() => {
            timer = null;
            lastCall = Date.now();
            fn(...args);
          }, throttle - (now - lastCall));
        }
      };
    }
    
    entries.push({ fn: handler, once, throttle });
    return {
      off: () => {
        const arr = this._subs.get(eventName);
        if (!arr) return;
        const idx = arr.findIndex(e => e.fn === handler);
        if (idx !== -1) arr.splice(idx, 1);
      }
    };
  }

  /** Subscribe once */
  once(eventName, fn) {
    return this.on(eventName, fn, { once: true });
  }

  /** Unsubscribe a specific function */
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

  /** Emit an event */
  emit(eventName, payload) {
    const entries = this._subs.get(eventName);
    if (!entries) return;
    
    // Take snapshot and clear once listeners
    const toCall = entries.filter(e => !e.once);
    const onceToCall = entries.filter(e => e.once);
    
    // Remove once listeners
    this._subs.set(eventName, toCall);
    
    // Call listeners
    [...onceToCall, ...toCall].forEach(({ fn }) => {
      try { fn(payload); }
      catch (err) { console.error(`[EventBus] Error in ${eventName}:`, err); }
    });
  }

  /** Cleanup all listeners and timers */
  cleanupAll() {
    for (const [, entries] of this._subs) {
      entries.forEach(({ once }) => {
        // once listeners are already handled in emit, but just in case
        // we keep them in _subs until emit clears them
      });
    }
    this._subs.clear();
    
    for (const [, { timer }] of this._throttleTimers) {
      if (timer) clearTimeout(timer);
    }
    this._throttleTimers.clear();
  }

  /** Check if event has listeners */
  hasListeners(eventName) {
    return (this._subs.get(eventName) || []).length > 0;
  }
}

/** IPC Wrapper - Standardizes Tauri invoke with retries and error context */
class IPC {
  /**
   * @param {any} tauri - Tauri __TAURI__ object (defaults to window.__TAURI__)
   * @param {Object} [options] - { retries: number, retryInterval: number }
   */
  constructor(tauri = (typeof window !== 'undefined' ? window.__TAURI__ : null), 
            options = { retries: 2, retryInterval: 1000 }) {
    this._tauri = tauri;
    this._options = options;
  }

  /** Invoke with retry logic */
  async invoke(command, args = {}) {
    if (!this._tauri) {
      throw new Error('Tauri not available');
    }
    
    let lastError = null;
    for (let attempt = 0; attempt <= this._options.retries; attempt++) {
      try {
        return await this._tauri.core.invoke(command, args);
      } catch (err) {
        lastError = err;
        
        // Convert to TauriError if needed
        if (!(err instanceof TauriError)) {
          const tauriErr = new TauriError(
            err.message || String(err),
            err.code || undefined,
            err.payload || undefined
          );
          lastError = tauriErr;
        }
        
        // Don't retry on certain errors (invalid args, etc)
        if (err instanceof Error && 
            (err.message.includes('Invalid') || 
             err.message.includes('not found') ||
             err.name === 'ValidationError')) {
          break;
        }
        
        // Wait before retry (except on last attempt)
        if (attempt < this._options.retries) {
          await new Promise(resolve => 
            setTimeout(resolve, this._options.retryInterval));
        }
      }
    }
    throw lastError;
  }

  /** Bind a command for partial application */
  bind(command) {
    return (args) => this.invoke(command, args);
  }

  /** Listen to a Tauri event from the backend */
  listen(eventName, callback) {
    if (!this._tauri || !this._tauri.event || !this._tauri.event.listen) {
      throw new Error('Tauri event listener not available');
    }
    const unlisten = this._tauri.event.listen(eventName, (event) => {
      callback(event.payload);
    });
    return {
      off: () => {
        try { unlisten(); } catch (e) { /* ignore cleanup errors */ }
      }
    };
  }
}

// Export all classes and functions as named exports
export { EventBus, IPC, TauriError, AppError, IPCError };