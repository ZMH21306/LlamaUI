// events.mjs - Event contract between frontend and backend
export const EVENTS = {
  // Service lifecycle
  SERVER_LOG: 'server-log',
  SERVER_STATUS: 'server-status',
  SERVER_METRICS: 'server-metrics',
  
  // Auto-detection / initialization
  DETECT_STEP: 'detect-step',
  DETECT_COMPLETE: 'detect-complete',
  
  // Downloads
  DOWNLOAD_STATE: 'download-state',
  DOWNLOAD_PROGRESS: 'download-progress',
  
  // Updates
  UPDATE_STATE: 'update-state',
  UPDATE_PROGRESS: 'update-progress',
  
  // Hardware / models
  GPU_INFO: 'gpu-info',
  MODEL_SELECTED: 'model-selected',
  
  // HuggingFace store window
  HF_DOWNLOAD_PROGRESS: 'hf-download-progress',
  
   // UI intents (no IPC)
  UI_START_REQUESTED: 'ui:start-requested',
  UI_STOP_REQUESTED: 'ui:stop-requested',
  UI_RESTART_REQUESTED: 'ui:restart-requested',
  UI_CONFIG_DIRTY: 'ui:config-dirty',
  UI_DOWNLOAD_REQUESTED: 'ui:download-requested',
  UI_DOWNLOAD_CANCEL: 'ui:download-cancelled',
  UI_UPDATE_CHECK: 'ui:update-check',
  UI_INIT_REQUESTED: 'ui:init-requested',
  UI_THEME_CHANGE: 'ui:theme-change',
  UI_SPLITTER_RESIZE: 'ui:splitter-resize',
  UI_EXPORT_LOGS_REQUESTED: 'ui:export-logs-requested',
  UI_CLEAR_LOGS_REQUESTED: 'ui:clear-logs-requested',
};

export default EVENTS;