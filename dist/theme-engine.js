// LlamaUI 主题引擎
// 基于 CSS 变量 + body.light-theme 类切换
'use strict';

class ThemeManager {
  constructor() {
    this.targetLight = false;
    this._bindSystemTheme();
    this._restorePreference();
  }

  setLightTheme(isLight) {
    this.targetLight = isLight;
    this._doSetLightTheme(isLight);
  }

  _doSetLightTheme(isLight) {
    if (isLight) {
      document.body.classList.add('light-theme');
    } else {
      document.body.classList.remove('light-theme');
    }
    this._savePreference(isLight);
    window.dispatchEvent(new CustomEvent('theme-change', {
      detail: { isLight }
    }));
  }

  _bindSystemTheme() {
    if (window.matchMedia) {
      const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
      const handler = (e) => {
        if (localStorage.getItem('llamaui-theme') === null) {
          this.setLightTheme(!e.matches);
        }
      };
      if (mediaQuery.addEventListener) {
        mediaQuery.addEventListener('change', handler);
      } else if (mediaQuery.addListener) {
        mediaQuery.addListener(handler);
      }
      if (localStorage.getItem('llamaui-theme') === null) {
        this.setLightTheme(!mediaQuery.matches);
      }
    }
  }

  _savePreference(isLight) {
    try {
      localStorage.setItem('llamaui-theme', isLight ? 'light' : 'dark');
    } catch (e) {}
  }

  _restorePreference() {
    try {
      const saved = localStorage.getItem('llamaui-theme');
      if (saved !== null) {
        this._doSetLightTheme(saved === 'light');
      }
    } catch (e) {}
  }

  isLightTheme() {
    return this.targetLight;
  }

  destroy() {}
}

const themeManager = new ThemeManager();
window.themeManager = themeManager;
