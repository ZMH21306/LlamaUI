// ui/serviceView.mjs - Service status UI renderer
import { MESSAGES } from '../constants/messages.mjs';

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
    const canStart = [status.Stopped, status.Crashed].includes(status.status);
    const canStop = [status.Starting, status.Running].includes(status.status);
    const canRestart = [status.Running].includes(status.status);
    
    els.startBtn.disabled = !canStart;
    els.stopBtn.disabled = !canStop;
    els.restartBtn.disabled = !canRestart;
  }
  
  function initEventListeners() {
    // Button click handlers - dispatch intents to event bus
    if (els.startBtn) {
      els.startBtn.addEventListener('click', () => {
        if (!els.startBtn.disabled) {
          bus.emit(EVENTS.UI_START_REQUESTED);
        }
      });
    }
    
    if (els.stopBtn) {
      els.stopBtn.addEventListener('click', () => {
        if (!els.stopBtn.disabled) {
          bus.emit(EVENTS.UI_STOP_REQUESTED);
        }
      });
    }
    
    if (els.restartBtn) {
      els.restartBtn.addEventListener('click', () => {
        if (!els.restartBtn.disabled) {
          bus.emit(EVENTS.UI_RESTART_REQUESTED);
        }
      });
    }
  }
  
  // Listen for actual service status updates from backend
  const statusUnsubscribe = bus.on(EVENTS.SERVER_STATUS, (status) => {
    stateKey.setState(status);
  });
  
  // Public API
  return {
    /** Clean up listeners and subscriptions */
    destroy() {
      unsubscribe();
      statusUnsubscribe.off();
      
      // Remove event listeners
      if (els.startBtn) els.startBtn.removeEventListener('click', els.startBtn._clickHandler);
      if (els.stopBtn) els.stopBtn.removeEventListener('click', els.stopBtn._clickHandler);
      if (els.restartBtn) els.restartBtn.removeEventListener('click', els.restartBtn._clickHandler);
    },
    
    /** Force a re-render */
    refresh() {
      render(undefined, stateKey.get());
    }
  };
}

// Helper function (will be replaced by import)
function formatElapsedMs(ms) {
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const ss = Math.floor(s % 60);
  return `${m}分${ss}秒`;
}

/** Default export */
export default createServiceView;