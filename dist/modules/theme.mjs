// theme.mjs - Simple theme manager
import { MESSAGES } from './constants/messages.mjs';

/**
 * Theme Manager - Handles light/dark theme switching
 */
export class ThemeManager {
  /**
   * @param {Object} [options] - Configuration options
   * @param {string} [options.storageKey] - Key for storing theme preference
   */
  constructor(options = {}) {
    this.storageKey = options.storageKey || 'llamaui-theme';
    this.root = document.documentElement;
    
    // Initialize
    this.restorePreference();
    this.bindSystemTheme();
  }
  
  /** Get current theme */
  get isLight() {
    return this._isLight ?? false;
  }
  
  /** Set theme explicitly */
  setLightTheme(isLight) {
    if (this._isLight === isLight) return;
    this._isLight = isLight;
    this.applyTheme();
    this.savePreference(isLight);
  }
  
  /** Toggle theme */
  toggle() {
    this.setLightTheme(!this.isLight);
  }
  
  /** Apply theme by setting CSS variables */
  applyTheme() {
    if (this.isLight) {
      // Light theme colors
      this.root.style.setProperty('--bg-0', '#f3f4f7');
      this.root.style.setProperty('--bg-1', '#ffffff');
      this.root.style.setProperty('--bg-2', '#f8f9fc');
      this.root.style.setProperty('--bg-3', '#eef1f6');
      this.root.style.setProperty('--text-1', '#1a202c');
      this.root.style.setProperty('--text-2', '#4a5568');
      this.root.style.setProperty('--text-3', '#8a94a6');
      this.root.style.setProperty('--accent', '#2d5fd3');
      this.root.style.setProperty('--accent-2', '#4a7fff');
      document.body.classList.add('light-theme');
      document.body.classList.remove('dark-theme');
    } else {
      // Dark theme colors
      this.root.style.setProperty('--bg-0', '#08090e');
      this.root.style.setProperty('--bg-1', '#0d0f16');
      this.root.style.setProperty('--bg-2', '#12151d');
      this.root.style.setProperty('--bg-3', '#181c26');
      this.root.style.setProperty('--text-1', '#e2e6ef');
      this.root.style.setProperty('--text-2', '#9aa3b8');
      this.root.style.setProperty('--text-3', '#5f6a7e');
      this.root.style.setProperty('--accent', '#5b8def');
      this.root.style.setProperty('--accent-2', '#7c9fff');
      document.body.classList.add('dark-theme');
      document.body.classList.remove('light-theme');
    }
  }
  
  /** Save preference to localStorage */
  savePreference(isLight) {
    try {
      localStorage.setItem(this.storageKey, isLight ? 'light' : 'dark');
    } catch (e) {
      console.warn('[ThemeManager] Failed to save preference:', e);
    }
  }
  
  /** Restore preference from localStorage */
  restorePreference() {
    try {
      const saved = localStorage.getItem(this.storageKey);
      if (saved === 'light' || saved === 'dark') {
        this._isLight = saved === 'light';
        this.applyTheme();
        return;
      }
    } catch (e) {
      console.warn('[ThemeManager] Failed to restore preference:', e);
    }
    
    // Fallback to system theme or dark
    if (window.matchMedia) {
      this._isLight = window.matchMedia('(prefers-color-scheme: light)').matches;
    } else {
      this._isLight = false; // Default to dark
    }
    this.applyTheme();
  }
  
  /** Bind to system theme changes */
  bindSystemTheme() {
    if (!window.matchMedia) return;
    
    const mediaQuery = window.matchMedia('(prefers-color-scheme: light)');
    const listener = (e) => {
      // Only change if user hasn't set a preference
      try {
        const saved = localStorage.getItem(this.storageKey);
        if (!saved) {
          this.setLightTheme(e.matches);
        }
      } catch (e) {
        // Ignore storage errors
      }
    };
    
    mediaQuery.addEventListener('change', listener);
    this._systemListener = listener;
  }
  
  /** Cleanup */
  destroy() {
    if (this._systemListener && window.matchMedia) {
      const mediaQuery = window.matchMedia('(prefers-color-scheme: light)');
      mediaQuery.removeEventListener('change', this._systemListener);
    }
  }
}

/** Default export */
export default ThemeManager;