// ui/logView.mjs - Log display with batching
import { MESSAGES } from '../constants/messages.mjs';

/**
 * Log View - Renders logs with efficient batching
 * @param {Object} deps - Dependencies
 * @param {Object} deps.store - State store
 * @param {Object} deps.bus - Event bus
 * @param {Object} deps.elements - DOM elements cache
 */
export function createLogView({ store, bus, elements }) {
  // Bind to logs store
  const logsKey = store.key('logs');
  const unsubscribe = logsKey.subscribe(renderLogs);
  
  // Cache elements
  const els = elements || {
    logsContainer: document.getElementById('logs'),
    autoScrollCheckbox: document.getElementById('autoScroll'),
    exportBtn: document.getElementById('exportLogs'),
    clearBtn: document.getElementById('clearLogs')
  };
  
  let isInitialized = false;
  let autoScroll = true;
  let logBatch = [];
  let batchTimer = null;
  const BATCH_DELAY_MS = 16; // ~60fps
  
  function renderLogs(_, current) {
    if (!els.logsContainer) return; // Not ready yet
    
    // Clear and render all groups
    els.logsContainer.innerHTML = '';
    
    const groups = current.groups || new Map();
    const order = current.order || [];
    
    // Render PLAIN group first
    if (groups.has('PLAIN')) {
      renderGroup('PLAIN', groups.get('PLAIN'));
    }
    
    // Render remaining groups in order
    for (const groupId of order) {
      if (groupId !== 'PLAIN' && groups.has(groupId)) {
        renderGroup(groupId, groups.get(groupId));
      }
    }
    
    // Auto-scroll
    if (autoScroll && els.logsContainer.lastChild) {
      els.logsContainer.scrollTop = els.logsContainer.scrollHeight;
    }
    
    // Init listeners on first render
    if (!isInitialized) {
      initEventListeners();
      isInitialized = true;
    }
  }
  
  function renderGroup(groupId, group) {
    if (!group || !els.logsContainer) return;
    
    // Group header
    const header = document.createElement('div');
    header.className = 'log-group-header';
    header.textContent = `${group.name || groupId} - ${group.status || 'idle'}`;
    els.logsContainer.appendChild(header);
    
    // Group content (if expanded)
    if (group.expanded) {
      const content = document.createElement('div');
      content.className = 'log-group-content';
      
      const entries = group.entries || [];
      entries.forEach(entry => {
        const entryEl = document.createElement('div');
        entryEl.className = 'log-entry';
        entryEl.textContent = `${entry.timestamp || ''} - ${entry.text || ''}`;
        content.appendChild(entryEl);
      });
      
      els.logsContainer.appendChild(content);
    }
    
    // Toggle on click
    header.addEventListener('click', () => {
      const logs = store.getState('logs');
      const groupsCopy = new Map(logs.groups || []);
      const groupCopy = { ...(groupsCopy.get(groupId) || {}) };
      groupCopy.expanded = !groupCopy.expanded;
      groupsCopy.set(groupId, groupCopy);
      
      store.setState({ groups: groupsCopy }, 'logs');
    });
  }
  
  function initEventListeners() {
    // Auto-scroll checkbox
    if (els.autoScrollCheckbox) {
      els.autoScrollCheckbox.addEventListener('change', (e) => {
        autoScroll = e.target.checked;
      });
    }
    
    // Export logs button
    if (els.exportBtn) {
      els.exportBtn.addEventListener('click', () => {
        bus.emit(EVENTS.UI_EXPORT_LOGS_REQUESTED);
      });
    }
    
    // Clear logs button
    if (els.clearBtn) {
      els.clearBtn.addEventListener('click', () => {
        bus.emit(EVENTS.UI_CLEAR_LOGS_REQUESTED);
      });
    }
  }
  
  // Listen for log updates from backend
  const logUnsubscribe = bus.on(EVENTS.SERVER_LOG, (logEntry) => {
    logBatch.push(logEntry);
    
    if (batchTimer) clearTimeout(batchTimer);
    batchTimer = setTimeout(() => {
      const logs = store.getState('logs');
      const plainGroupId = 'PLAIN';
      
      let plainGroup = logs.groups ? logs.groups.get(plainGroupId) : null;
      if (!plainGroup) {
        plainGroup = {
          id: plainGroupId,
          name: '系统日志',
          status: 'running',
          expanded: true,
          entries: []
        };
      }
      
      const newEntries = [...(plainGroup.entries || []), ...logBatch];
      plainGroup.entries = newEntries.slice(-5000); // Keep only last 5000
      
      const groupsCopy = new Map(logs.groups || []);
      groupsCopy.set(plainGroupId, plainGroup);
      
      let orderCopy = [...(logs.order || [])];
      if (!orderCopy.includes(plainGroupId)) {
        orderCopy.unshift(plainGroupId);
      }
      
      store.setState({ groups: groupsCopy, order: orderCopy }, 'logs');
      
      logBatch = [];
      batchTimer = null;
    }, BATCH_DELAY_MS);
  });
  
  return {
    destroy() {
      unsubscribe();
      logUnsubscribe.off();
      if (batchTimer) {
        clearTimeout(batchTimer);
        batchTimer = null;
      }
    },
    
    refresh() {
      renderLogs(undefined, logsKey.get());
    }
  };
}

/** Default export */
export default createLogView;