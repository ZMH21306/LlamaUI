// modules/index.mjs - Module entry point
import { EventBus, IPC, TauriError, AppError, IPCError } from './bus.mjs';
import createStore from './store.mjs';
import { createMachine, mergeConfigs } from './stateMachine.mjs';
import EVENTS from './constants/events.mjs';
import { MESSAGES, formatTime, formatElapsedMs } from './constants/messages.mjs';

export {
  // Core infrastructure
  EventBus,
  IPC,
  createStore,
  createMachine,
  mergeConfigs,
  
  // Constants
  EVENTS,
  MESSAGES,
  formatTime,
  formatElapsedMs,
  
  // Errors
  TauriError,
  AppError,
  IPCError
};

// Default export
export default {
  EventBus,
  IPC,
  createStore,
  createMachine,
  mergeConfigs,
  EVENTS,
  MESSAGES,
  formatTime,
  formatElapsedMs,
  TauriError,
  AppError,
  IPCError
};