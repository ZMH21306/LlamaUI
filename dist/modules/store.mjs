// store.mjs - Immutable state store with key-based subscriptions
import { MESSAGES } from './constants/messages.mjs';

/** Deep clone utility */
function deepClone(value) {
  return JSON.parse(JSON.stringify(value));
}

/** Get nested value by dot-notation path */
function getByPath(obj, path) {
  if (!path) return obj;
  return path.split('.').reduce((acc, part) => 
    acc == null ? undefined : acc[part], obj);
}

/** Set nested value by dot-notation path */
function setByPath(obj, path, value) {
  if (!path) {
    // Can't replace root object easily, caller should handle this
    return false;
  }
  
  const parts = path.split('.');
  const last = parts.pop();
  let target = obj;
  
  for (const part of parts) {
    if (target[part] == null || typeof target[part] !== 'object') {
      target[part] = {};
    }
    target = target[part];
  }
  
  target[last] = value;
  return true;
}

/**
 * Create a state store
 * @param {Object} initialState - Initial state object
 * @returns {{ getState: Function, setState: Function, subscribe: Function, key: Function, dispose: Function }}
 */
export function createStore(initialState = {}) {
  let state = deepClone(initialState);
  
  /** @type {Map<string|undefined, Set<Function>>} */
  const subscribers = new Map();
  
  /**
   * Notify subscribers of a change
   * @param {*} prevState 
   * @param {string|undefined} key - Changed key (undefined for root)
   */
  function notify(prevState, key) {
    // Notify key-specific subscribers
    const keySubs = subscribers.get(key);
    if (keySubs) {
      keySubs.forEach(fn => {
        try { fn(prevState, deepClone(getByPath(state, key))); }
        catch (e) { console.error('[Store] Notification error:', e); }
      });
    }
    
    // Notify root subscribers (key === undefined)
    const rootSubs = subscribers.get(undefined);
    if (rootSubs) {
      rootSubs.forEach(fn => {
        try { fn(prevState, deepClone(state)); }
        catch (e) { console.error('[Store] Root notification error:', e); }
      });
    }
  }
  
  const store = {
    /**
     * Get state (deep cloned)
     * @param {string} [key] - Dot-notation path, omit for root
     * @returns {*} State value
     */
    getState(key) {
      return deepClone(getByPath(state, key));
    },
    
    /**
     * Set state
     * @param {*} valueOrMutator - New value or function(prev) => newValue
     * @param {string} [key] - Dot-notation path, omit for root
     * @returns {*} New state value
     */
    setState(valueOrMutator, key) {
      const prevState = deepClone(state);
      
      if (typeof valueOrMutator === 'function') {
        if (key === undefined) {
          state = valueOrMutator(deepClone(state));
        } else {
          const current = getByPath(state, key);
          const newValue = valueOrMutator(current);
          setByPath(state, key, newValue);
        }
      } else {
        if (key === undefined) {
          state = valueOrMutator;
        } else {
          setByPath(state, key, valueOrMutator);
        }
      }
      
      notify(prevState, key);
      return deepClone(getByPath(state, key));
    },
    
    /**
     * Subscribe to state changes
     * @param {string} key - Dot-notation path (undefined for root)
     * @param {Function} fn - Callback(prev, current)
     * @returns {{ unsubscribe: Function }}
     */
    subscribe(key, fn) {
      if (!subscribers.has(key)) {
        subscribers.set(key, new Set());
      }
      subscribers.get(key).add(fn);
      
      // Immediate call with current value
      try {
        const current = key === undefined ? deepClone(state) : 
                       deepClone(getByPath(state, key));
        fn(undefined, current);
      } catch (e) {
        console.error('[Store] Initial subscription error:', e);
      }
      
      return {
        unsubscribe: () => {
          const set = subscribers.get(key);
          if (set) set.delete(fn);
        }
      };
    },
    
    /**
     * Get a key-specific store proxy
     * @param {string} key - Dot-notation path
     * @returns {{ get: Function, set: Function, subscribe: Function }}
     */
    key(keyPath) {
      return {
        get: () => store.getState(keyPath),
        set: (value) => store.setState(value, keyPath),
        subscribe: (fn) => store.subscribe(keyPath, fn)
      };
    },
    
    /** Clean up all subscribers */
    dispose() {
      subscribers.clear();
    }
  };
  
  return store;
}

/** Default export */
export default createStore;