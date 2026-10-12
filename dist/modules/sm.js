/**
 * 有限状态机（创建型工厂）
 *
 * 设计要点：
 *  - 每个状态一个对象，状态内的 on.<事件> 是转换/副作用处理函数。
 *  - 处理函数签名：handler(state, payload, machine)
 *   - 返回字符串：发生状态转换（目标状态名）
 *   - 返回 undefined：留在当前状态（但可执行副作用）
 *  - onEnter/onExit：进入/离开状态时的副作用（如启动/停止 IPC 调用）。
 *  - 统一 emit：订阅者在状态变更后收到新状态。
 *
 * 典型用法：
 *   const service = createMachine({
 *     initialState: 'Stopped',
 *     on: {
 *       Stopped: {
 *         'start-requested': () => 'Starting',      // 状态转换
 *       },
 *     },
 *     onEnter: {
 *       Starting: (state, event, payload, machine) => {
 *         machine.setState('Starting');            // 先设状态（UI 立即反馈）
 *         invoke('start_server')
 *           .then((r) => machine.transition('server-status', r))
 *           .catch((err) => machine.transition('server-status', { status: 'Crashed', error: err }));
 *       },
 *     },
 *     onExit: {
 *       Starting: (state, event, payload, machine) => { /* 可选清理 */ },
 *     },
 *   });
 *
 * 状态机同时承担"拒绝非法转换"的责任：未定义的 事件 被静默忽略（不抛错），
 * 这样即使后端事件顺序异常，UI 也不会崩溃，只是保持当前状态。
 */

/**
 * @typedef {object} MachineConfig
 * @property {string} initialState
 * @property {Record<string, Record<string, (state, payload, machine)=>any>>} [on]
 *   on[当前状态名][事件名] = 处理函数
 * @property {Record<string, (state, event, payload, machine)=>void>} [onEnter]
 * @property {Record<string, (state, event, payload, machine)=>void>} [onExit]
 */

export function createMachine(config) {
  const { initialState, on = {}, onEnter = {}, onExit = {} } = config;
  let state = initialState;
  const subs = new Set();

  const machine = {
    /** 当前状态名 */
    get state() {
      return state;
    },

    /** 深拷贝的当前状态快照 */
    getSnapshot() {
      return state;
    },

    /**
     * 触发事件
     * @param {string} event
     * @param {*} [payload]
     */
    transition(event, payload) {
      const from = state;
      const transitions = on[from] || {};
      const handler = transitions[event];
      let next = from;

      if (handler) {
        try {
          const result = handler(state, event, payload, machine);
          if (typeof result === 'string') {
            next = result;
          }
        } catch (err) {
          console.error(`[StateMachine] handler error in state "${from}" event "${event}":`, err);
        }
      }

      if (next !== from) {
        if (onExit[from]) {
          try {
            onExit[from](state, event, payload, machine);
          } catch (err) {
            console.error(`[StateMachine] onExit error in state "${from}":`, err);
          }
        }
        state = next;
        if (onEnter[state]) {
          try {
            onEnter[state](state, event, payload, machine);
          } catch (err) {
            console.error(`[StateMachine] onEnter error in state "${state}":`, err);
          }
        }
        subs.forEach((fn) => fn(state));
      }
      return state;
    },

    /** 直接设状态（不触发 onEnter/onExit，用于内部流程） */
    setState(newState) {
      if (newState !== state) {
        state = newState;
        subs.forEach((fn) => fn(state));
      }
      return state;
    },

    /** 状态机配置快照（供外部组合） */
    get config() {
      return config;
    },

    /** 订阅状态变化 */
    on(fn) {
      subs.add(fn);
      return () => subs.delete(fn);
    },

    getState() {
      return state;
    },
    getInitialState() {
      return initialState;
    },
  };

  return machine;
}

/**
 * 组合两个状态机配置：child 覆盖 parent 的同名状态
 * @param {MachineConfig} parent
 * @param {MachineConfig} child
 */
export function mergeConfigs(parent, child) {
  return {
    initialState: child.initialState ?? parent.initialState,
    on: { ...parent.on, ...child.on },
    onEnter: { ...parent.onEnter, ...child.onEnter },
    onExit: { ...parent.onExit, ...child.onExit },
  };
}