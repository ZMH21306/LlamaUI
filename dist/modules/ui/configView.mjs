// ui/configView.mjs - Configuration form handler (simplified)
import { MESSAGES } from '../constants/messages.mjs';
import { formatTime } from '../constants/messages.mjs';

/**
 * Config View - Handles configuration form and persistence
 * @param {Object} deps - Dependencies
 * @param {Object} deps.store - State store
 * @param {Object} deps.bus - Event bus
 * @param {Object} deps.ipc - IPC wrapper
 * @param {Object} deps.elements - DOM elements cache
 */
export function createConfigView({ store, bus, ipc, elements }) {
  // Bind to config store
  const configKey = store.key('config');
  const unsubscribe = configKey.subscribe(renderConfig);
  
  // Cache elements
  const els = elements || {
    // Key form elements
    modelsDirInput: document.getElementById('modelsDirInput'),
    ctxSizeInput: document.getElementById('ctxSizeInput'),
    nGpuLayersInput: document.getElementById('nGpuLayersInput'),
    portInput: document.getElementById('portInput'),
    saveBtn: document.getElementById('saveBtn'),
    saveStatus: document.getElementById('saveStatus'),
    saveTime: document.getElementById('saveTime')
  };
  
  let isInitialized = false;
  let saveTimeout = null;
  const SAVE_DEBOUNCE_MS = 300;
  
  function renderConfig(_, current) {
    if (!els.modelsDirInput) return; // Not ready yet
    
    // Populate form from config
    els.modelsDirInput.value = current.modelsDir || '';
    els.ctxSizeInput.value = current.ctxSize || 2048;
    els.nGpuLayersInput.value = current.nGpuLayers || 0;
    els.portInput.value = current.port || 8080;
    
    // Update save status
    updateSaveStatus();
    
    // Initialize listeners on first render
    if (!isInitialized) {
      initEventListeners();
      isInitialized = true;
    }
  }
  
  function updateSaveStatus() {
    if (!els.saveStatus) return;
    
    const dirty = store.getState('configDirty');
    const saveState = store.getState('configSaveState');
    const saveTime = store.getState('configSaveTime');
    
    if (saveState === 'saving') {
      els.saveStatus.textContent = MESSAGES.configSaving;
      els.saveStatus.className = 'save-status saving';
    } else if (saveState === 'saved') {
      els.saveStatus.textContent = MESSAGES.configSaved;
      els.saveStatus.className = 'save-status saved';
      if (saveTime && els.saveTime) {
        els.saveTime.textContent = formatTime(saveTime);
      }
    } else if (saveState === 'error') {
      const error = store.getState('configSaveError');
      els.saveStatus.textContent = `${MESSAGES.configSaveFailed}: ${error || 'unknown'}`;
      els.saveStatus.className = 'save-status error';
    } else {
      // idle state
      if (dirty) {
        els.saveStatus.textContent = MESSAGES.configDirty;
        els.saveStatus.className = 'save-status dirty';
      } else {
        els.saveStatus.textContent = MESSAGES.configSaved;
        els.saveStatus.className = 'save-status saved';
        if (saveTime && els.saveTime) {
          els.saveTime.textContent = formatTime(saveTime);
        }
      }
    }
    
    // Update save button state
    if (els.saveBtn) {
      els.saveBtn.disabled = !(dirty && saveState !== 'saving');
    }
  }
  
  function scheduleSave() {
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(() => {
      performSave();
    }, SAVE_DEBOUNCE_MS);
  }
  
  async function performSave() {
    if (saveTimeout) {
      clearTimeout(saveTimeout);
      saveTimeout = null;
    }
    
    // Don't save if already saving
    const saveState = store.getState('configSaveState');
    if (saveState === 'saving') return;
    
    // Mark as saving
    store.setState({ configSaveState: 'saving', configSaveError: null }, 'configSaveState');
    
    try {
      // Gather current form values
      const config = {
        modelsDir: els.modelsDirInput.value.trim(),
        ctxSize: parseInt(els.ctxSizeInput.value) || 2048,
        nGpuLayers: parseInt(els.nGpuLayersInput.value) || 0,
        port: parseInt(els.portInput.value) || 8080
      };
      
      // Save via IPC
      await ipc.invoke('save_config', config);
      
      // Update state
      store.setState({
        configSaveState: 'saved',
        configSaveTime: Date.now(),
        configDirty: false,
        configSaveError: null
      }, 'configSaveState');
      
    } catch (err) {
      console.error('[ConfigView] Save failed:', err);
      store.setState({
        configSaveState: 'error',
        configSaveError: err.message || String(err)
      }, 'configSaveState');
    }
  }
  
  function initEventListeners() {
    // Form input listeners - mark dirty on change
    const inputs = [
      els.modelsDirInput,
      els.ctxSizeInput,
      els.nGpuLayersInput,
      els.portInput
    ];
    
    inputs.forEach(input => {
      if (input) {
        input.addEventListener('change', () => {
          store.setState({ configDirty: true }, 'configDirty');
          scheduleSave();
        });
        input.addEventListener('input', () => {
          store.setState({ configDirty: true }, 'configDirty');
          scheduleSave();
        });
      }
    });
    
    // Save button click
    if (els.saveBtn) {
      els.saveBtn.addEventListener('click', () => {
        performSave();
      });
    }
  }
  
  // Listen for config changes from backend
  const configUnsubscribe = bus.on(EVENTS.CONFIG_LOADED, (config) => {
    configKey.setState(config);
  });
  
  return {
    /** Clean up */
    destroy() {
      unsubscribe();
      configUnsubscribe.off();
      
      if (saveTimeout) {
        clearTimeout(saveTimeout);
        saveTimeout = null;
      }
    },
    
    /** Force refresh */
    refresh() {
      renderConfig(undefined, configKey.get());
    }
  };
}

/** Default export */
export default createConfigView;