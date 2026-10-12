// stateMachine.mjs - Finite state machine factory

/**
 * Create a finite state machine
 * @param {Object} config - Machine configuration
 * @param {string} config.initialState - Starting state
 * @param {Object} [config.on] - State transitions: on[state][event] = handler
 * @param {Object} [config.onEnter] - Side effects on state entry
 * @param {Object} [config.onExit] - Side effects on state exit
 * @returns {{ state: string, transition: Function, setState: Function, on: Function, getState: Function }}
 */
export function createMachine(config) {
  const { initialState, on = {}, onEnter = {}, onExit = {} } = config;
  let state = initialState;
  const subscribers = new Set();
  
  const machine = {
    /** Current state name */
    get state() {
      return state;
    },
    
    /** Get snapshot of current state (string) */
    getState() {
      return state;
    },
    
    /**
     * Transition on an event
     * @param {string} event - Event name
     * @param {*} payload - Event payload
     * @returns {string} New state
     */
    transition(event, payload) {
      const fromState = state;
      const transitions = on[fromState] || {};
      const handler = transitions[event];
      let nextState = fromState;
      
      if (handler) {
        try {
          const result = handler(state, event, payload, machine);
          if (typeof result === 'string') {
            nextState = result;
          }
          // If handler returns undefined/false/etc, stay in current state
        } catch (err) {
          console.error(`[StateMachine] Handler error in state "${fromState}" event "${event}":`, err);
          // Stay in current state on error
        }
      }
      
      // State changed - run lifecycle hooks
      if (nextState !== fromState) {
        // Exit current state
        if (onExit[fromState]) {
          try {
            onExit[fromState](state, event, payload, machine);
          } catch (err) {
            console.error(`[StateMachine] onExit error in state "${fromState}":`, err);
          }
        }
        
        // Enter new state
        state = nextState;
        if (onEnter[state]) {
          try {
            onEnter[state](state, event, payload, machine);
          } catch (err) {
            console.error(`[StateMachine] onEnter error in state "${state}":`, err);
          }
        }
        
        // Notify subscribers
        subscribers.forEach(fn => {
          try { fn(state); }
          catch (e) { console.error('[StateMachine] Subscription error:', e); }
        });
      }
      
      return state;
    },
    
    /**
     * Set state directly (bypassing lifecycle hooks)
     * @param {string} newState
     * @returns {string} New state
     */
    setState(newState) {
      if (newState !== state) {
        state = newState;
        subscribers.forEach(fn => {
          try { fn(state); }
          catch (e) { console.error('[StateMachine] setState subscription error:', e); }
        });
      }
      return state;
    },
    
    /** Subscribe to state changes */
    on(fn) {
      subscribers.add(fn);
      return () => subscribers.delete(fn);
    },
    
    /** Get initial state */
    getInitialState() {
      return initialState;
    }
  };
  
  return machine;
}