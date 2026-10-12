// ui/serviceView.mjs - Service status UI renderer
import { MESSAGES, formatElapsedMs } from '../constants/messages.mjs';
import { EVENTS } from '../constants/events.mjs';

/**
 * Service View - Renders service status and controls
 * @param {Object} deps - Dependencies
 * @param {Object} deps.store - State store
 * @param {Object} deps.bus - Event bus
 * @param {Object} deps.ipc - IPC wrapper
 * @param {Object} deps.elements - DOM elements cache
 */
export function createServiceView({ store, bus, ipc, elements }) {
  // Bind to store for state changes
  const stateKey = store.key('service');
  const unsubscribe = stateKey.subscribe(render);
  
  // Cache elements if not provided
  const els = elements || {
    statusText: document.getElementById('statusText'),
    startBtn: document.getElementById('startBtn'),
    stopBtn: document.getElementById('stopBtn'),
    restartBtn: document.getElementById('restartBtn'),
    metricPid: document.getElementById('metricPid'),
    metricUptime: document.getElementById('metricUptime')
  };
  
  let isInitialized = false;
  
  function render(_, current) {
    if (!els.statusText) return; // Not in DOM yet
    
    // Update status text
    const statusMap = {
      Stopped: MESSAGES.serviceStopped,
      Starting: MESSAGES.serviceStarting,
      Running: MESSAGES.serviceRunning,
      Stopping: MESSAGES.serviceStopping,
      Crashed: MESSAGES.serviceCrashed
    };
    els.statusText.textContent = statusMap[current.status] || MESSAGES.serviceUnknown;
    
    // Update button states
    updateButtonStates(current);
    
    // Update metrics if available
    if (current.pid !== null) {
      els.metricPid.textContent = current.pid;
    } else {
      els.metricPid.textContent = MESSAGES.empty;
    }
    
    if (current.startedAt !== null) {
      const uptime = Date.now() - current.startedAt;
      els.metricUptime.textContent = formatElapsedMs(uptime);
    } else {
      els.metricUptime.textContent = MESSAGES.empty;
    }
    
    // Initialize event listeners on first render
    if (!isInitialized) {
      initEventListeners();
      isInitialized = true;
    }
  }
  
  function updateButtonStates(status) {
    if (!els.startBtn) return;
    
    // Determine button states based on service status
    const canStart = ['Stopped', 'Crashed'].includes(status.status || status);
    const canStop = ['Starting', 'Running'].includes(status.status || status);
    const canRestart = ['Running'].includes(status.status || status);
    
    els.startBtn.disabled = !canStart;
    els.stopBtn.disabled = !canStop;
    els.restartBtn.disabled = !canRestart;
  }
  
  function initEventListeners() {
    // Button click handlers - dispatch intents to event bus
    if (els.startBtn) {
      const startHandler = () => {
        if (!els.startBtn.disabled) {
          bus.emit(EVENTS.UI_START_REQUESTED);
        }
      };
      els.startBtn.addEventListener('click', startHandler);
      els.startBtn._clickHandler = startHandler;
    }
    
    if (els.stopBtn) {
      const stopHandler = () => {
        if (!els.stopBtn.disabled) {
          bus.emit(EVENTS.UI_STOP_REQUESTED);
        }
      };
      els.stopBtn.addEventListener('click', stopHandler);
      els.stopBtn._clickHandler = stopHandler;
    }
    
    if (els.restartBtn) {
      const restartHandler = () => {
        if (!els.restartBtn.disabled) {
          // 直接调用 IPC 重启命令
          ipc.invoke('restart_server')
            .catch(err => console.error('[bootstrap] restart failed:', err));
        }
      };
      els.restartBtn.addEventListener('click', restartHandler);
      els.restartBtn._clickHandler = restartHandler;
    }
  }
  
  // Public API
  return {
    /** Clean up listeners and subscriptions */
    destroy() {
      unsubscribe();
    },
    
    /** Force a re-render */
    refresh() {
      render(undefined, stateKey.get());
    }
  };
}

/** Default export */
export default createServiceView;