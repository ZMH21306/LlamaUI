/**
 * 单一状态仓库（StateStore）
 *
 * 原则：
 *  - 唯一数据源：所有 UI 状态集中在一个 store。
 *  - 不可变快照：getState() 返回深拷贝，防止外部直接篡改。
 *  - 细粒度订阅：subscribe(key, fn) 只在 key 路径变化时触发。
 *  - 批量更新：setState(mutator) 触发一次通知，避免多次渲染。
 */

function clone(value) {
  if (typeof structuredClone === 'function') {
    return structuredClone(value);
  }
  // 降级：JSON 克隆（丢失函数/Date→字符串）
  return JSON.parse(JSON.stringify(value));
}

function getNested(obj, key) {
  if (!key) return obj;
  return key.split('.').reduce((acc, k) => (acc == null ? undefined : acc[k]), obj);
}

function setNested(obj, key, value) {
  const parts = key.split('.');
  const last = parts.pop();
  let target = obj;
  for (const p of parts) {
    if (target[p] == null || typeof target[p] !== 'object') {
      target[p] = {};
    }
    target = target[p];
  }
  target[last] = value;
}

/**
 * 创建状态仓库
 * @param {object} [initialState] 初始状态
 */
export function createStore(initialState = {}) {
  let state = clone(initialState);
  /** @type {Map<string|undefined, Set<(prev:any,next:any)=>void>>} */
  const subs = new Map();

  function notify(prev, key) {
    // 通知 key 订阅者
    const bucket = subs.get(key);
    if (bucket) {
      bucket.forEach((fn) => fn(prev, clone(state)));
    }
    // 通知全局订阅者（key === undefined）
    const all = subs.get(undefined);
    if (all) {
      all.forEach((fn) => fn(prev, clone(state)));
    }
  }

  const store = {
    /**
     * 获取状态（深拷贝）。
     * @param {string} [key] 点号路径，省略返回整棵树
     */
    getState(key) {
      return clone(getNested(state, key));
    },

    /**
     * 设置状态。valueOrMutator 可为新值或 (draft) => newValue 的函数。
     * @param {*} valueOrMutator
     * @param {string} [key] 点号路径，省略替换整棵树
     */
    setState(valueOrMutator, key) {
      const prev = clone(state);
      if (typeof valueOrMutator === 'function') {
        if (key === undefined) {
          state = valueOrMutator(clone(state));
        } else {
          setNested(state, key, valueOrMutator(getNested(state, key)));
        }
      } else if (key === undefined) {
        state = valueOrMutator;
      } else {
        setNested(state, key, valueOrMutator);
      }
      notify(prev, key);
      return clone(state);
    },

    /** 订阅 key 路径变化，立即触发一次当前值 */
    subscribe(key, fn) {
      if (!subs.has(key)) {
        subs.set(key, new Set());
      }
      subs.get(key).add(fn);
      // 立即回放当前值
      try {
        fn(undefined, clone(getNested(state, key)));
      } catch (e) {
        console.error('[Store] subscribe initial error:', e);
      }
      return {
        unsubscribe() {
          const s = subs.get(key);
          if (s) s.delete(fn);
        },
      };
    },

    /** 获取 key 代理对象（get/set 直通 store） */
    key(k) {
      return new KeyStore(store, k);
    },

    /** 一次性清理全部订阅 */
    dispose() {
      subs.clear();
    },
  };

  return store;
}

/** KeyStore：让模块像访问字段一样读写 store[key] */
export class KeyStore {
  constructor(store, key) {
    this._store = store;
    this._key = key;
    return new Proxy(this, {
      get(target, prop) {
        if (prop in target) {
          return target[prop];
        }
        // 代理到 store.getState(key)
        const v = target._store.getState(target._key);
        return v == null ? undefined : v[prop];
      },
      set(target, prop, value) {
        const draft = target._store.getState(target._key) || {};
        draft[prop] = value;
        target._store.setState(draft, target._key);
        return true;
      },
    });
  }

  get get() {
    return this._store.getState(this._key);
  }
  set set(value) {
    this._store.setState(value, this._key);
  }
}

export default createStore;