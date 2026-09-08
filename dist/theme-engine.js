// ============================================================
// LlamaUI 主题引擎 v3
// 基于 CSS 变量 + body.light-theme 类的简洁主题切换系统
// ============================================================
'use strict';

// ============= 基础色板定义（HSL） =============
// 新设计：Obsidian Abyss（暗色）/ Alabaster Studio（亮色）
// 主色相：220（青金石蓝）/ 辅助：262（紫罗兰）
// 语义色相：150（success）/ 39（warning）/ 0（danger）/ 212（info）
const BASE_PALETTE = {
  primaryH: 220,      // 青金石蓝主色相
  secondaryH: 262,    // 紫罗兰辅助色相
  successH: 150,      // 翡翠绿
  warningH: 39,       // 琥珀
  dangerH: 0,         // 红
  infoH: 212,         // 蓝
};

function generatePalette(isLight) {
  const { primaryH, secondaryH, successH, warningH, dangerH, infoH } = BASE_PALETTE;
  if (isLight) {
    return {
      bg0: { h: 225, s: 10, l: 96 },
      bg1: { h: 225, s: 6, l: 100 },
      bg2: { h: 225, s: 8, l: 97 },
      bg3: { h: 225, s: 10, l: 93 },
      bgInput: { h: 225, s: 6, l: 100 },
      bgElevated: { h: 225, s: 6, l: 100 },
      border: { h: 225, s: 12, l: 84 },
      borderStrong: { h: 225, s: 14, l: 74 },
      borderSubtle: { h: 225, s: 8, l: 95 },
      text1: { h: 225, s: 30, l: 12 },
      text2: { h: 225, s: 18, l: 32 },
      text3: { h: 225, s: 12, l: 58 },
      textOnAccent: { h: 0, s: 0, l: 100 },
      accent: { h: 225, s: 78, l: 55 },
      accent2: { h: 225, s: 80, l: 68 },
      accentSoft: { h: 225, s: 78, l: 55, a: 0.12 },
      accentPurple: { h: 262, s: 70, l: 55 },
      accentPurpleSoft: { h: 262, s: 70, l: 55, a: 0.12 },
      success: { h: 155, s: 60, l: 35 },
      successSoft: { h: 155, s: 60, l: 35, a: 0.12 },
      warning: { h: 35, s: 80, l: 45 },
      warningSoft: { h: 35, s: 80, l: 45, a: 0.12 },
      danger: { h: 5, s: 75, l: 48 },
      dangerSoft: { h: 5, s: 75, l: 48, a: 0.12 },
      info: { h: 220, s: 85, l: 55 },
      infoSoft: { h: 220, s: 85, l: 55, a: 0.12 },
    };
  } else {
    return {
      bg0: { h: 225, s: 30, l: 4 },
      bg1: { h: 225, s: 28, l: 7 },
      bg2: { h: 225, s: 26, l: 10 },
      bg3: { h: 225, s: 24, l: 14 },
      bgInput: { h: 225, s: 28, l: 6 },
      bgElevated: { h: 225, s: 30, l: 11 },
      border: { h: 225, s: 22, l: 16 },
      borderStrong: { h: 225, s: 24, l: 24 },
      borderSubtle: { h: 225, s: 20, l: 10 },
      text1: { h: 225, s: 14, l: 92 },
      text2: { h: 225, s: 12, l: 62 },
      text3: { h: 225, s: 10, l: 44 },
      textOnAccent: { h: 0, s: 0, l: 100 },
      accent: { h: 225, s: 70, l: 62 },
      accent2: { h: 225, s: 72, l: 74 },
      accentSoft: { h: 225, s: 70, l: 62, a: 0.14 },
      accentPurple: { h: 262, s: 65, l: 68 },
      accentPurpleSoft: { h: 262, s: 65, l: 68, a: 0.14 },
      success: { h: 155, s: 65, l: 62 },
      successSoft: { h: 155, s: 65, l: 62, a: 0.14 },
      warning: { h: 39, s: 85, l: 60 },
      warningSoft: { h: 39, s: 85, l: 60, a: 0.14 },
      danger: { h: 0, s: 80, l: 65 },
      dangerSoft: { h: 0, s: 80, l: 65, a: 0.14 },
      info: { h: 220, s: 80, l: 65 },
      infoSoft: { h: 220, s: 80, l: 65, a: 0.14 },
    };
  }
}

function hslToRgb(h, s, l) {
  h = ((h % 360) + 360) % 360 / 360;
  s = Math.max(0, Math.min(1, s / 100));
  l = Math.max(0, Math.min(1, l / 100));
  if (s === 0) {
    const gray = Math.round(l * 255);
    return 'rgb(' + gray + ', ' + gray + ', ' + gray + ')';
  }
  const hue2rgb = function(p, q, t) {
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1/6) return p + (q - p) * 6 * t;
    if (t < 1/2) return q;
    if (t < 2/3) return p + (q - p) * (2/3 - t) * 6;
    return p;
  };
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const r = hue2rgb(p, q, h + 1/3);
  const g = hue2rgb(p, q, h);
  const b = hue2rgb(p, q, h - 1/3);
  return 'rgb(' + Math.round(r * 255) + ', ' + Math.round(g * 255) + ', ' + Math.round(b * 255) + ')';
}

function hslToRgba(h, s, l, a) {
  const rgb = hslToRgb(h, s, l).replace('rgb(', '').replace(')', '');
  return 'rgba(' + rgb + ', ' + a + ')';
}

class ThemeManager {
  constructor() {
    this.targetLight = false;
    this.currentLight = false;
    this._debounceTimer = null;
    this._debounceMs = 300;
    this._pendingLight = null;
    this._darkPalette = null;
    this._lightPalette = null;
    this._bindSystemTheme();
    this._restorePreference();
  }

  setLightTheme(isLight) {
    if (this.targetLight === isLight) return;
    this._pendingLight = isLight;
    if (this._debounceTimer) clearTimeout(this._debounceTimer);
    this._debounceTimer = setTimeout(() => {
      const pending = this._pendingLight;
      this._pendingLight = null;
      if (pending !== null) this._doSetLightTheme(pending);
    }, this._debounceMs);
  }

  _doSetLightTheme(isLight) {
    if (this.targetLight === isLight && this.currentLight === isLight) return;
    this.targetLight = isLight;
    this.currentLight = isLight;

    const body = document.body;

    // 1) 先添加过渡状态类，让浏览器准备好过渡效果
    //    这确保 CSS 变量变化时有动画，而不是瞬间切换
    body.classList.add('theme-transitioning');

    // 2) 创建过渡遮罩层，提供视觉过渡反馈
    let overlay = document.querySelector('.theme-transition-overlay');
    if (!overlay) {
      overlay = document.createElement('div');
      overlay.className = 'theme-transition-overlay';
      body.appendChild(overlay);
    }

    // 3) 强制重排以确保 transition 类被应用（关键步骤）
    //    否则 CSS 变量会立即变化，跳过过渡动画
    void body.offsetHeight;

    // 3) 切换主题类（添加新类，移除旧类）
    body.classList.remove('dark-theme', 'light-theme');
    body.classList.add(isLight ? 'light-theme' : 'dark-theme');

    // 4) 应用 CSS 变量（新值会通过 transition 平滑过渡）
    this._applyColors();
    this._savePreference(isLight);

    // 5) 动画结束后移除过渡状态，避免影响其他交互
    //    350ms 与 CSS 中的 --dur-slow 保持一致
    setTimeout(() => {
      body.classList.remove('theme-transitioning');
    }, 350);

    window.dispatchEvent(new CustomEvent('theme-change', {
      detail: { isLight, progress: 1 }
    }));
  }

  _applyColors() {
    const dark = this._getDarkPalette();
    const light = this._getLightPalette();
    const src = this.targetLight ? light : dark;
    const root = document.documentElement.style;

    root.setProperty('--bg-0', hslToRgb(src.bg0.h, src.bg0.s, src.bg0.l));
    root.setProperty('--bg-1', hslToRgb(src.bg1.h, src.bg1.s, src.bg1.l));
    root.setProperty('--bg-2', hslToRgb(src.bg2.h, src.bg2.s, src.bg2.l));
    root.setProperty('--bg-3', hslToRgb(src.bg3.h, src.bg3.s, src.bg3.l));
    root.setProperty('--bg-input', hslToRgb(src.bgInput.h, src.bgInput.s, src.bgInput.l));
    root.setProperty('--bg-elevated', hslToRgb(src.bgElevated.h, src.bgElevated.s, src.bgElevated.l));
    root.setProperty('--border', hslToRgb(src.border.h, src.border.s, src.border.l));
    root.setProperty('--border-strong', hslToRgb(src.borderStrong.h, src.borderStrong.s, src.borderStrong.l));
    root.setProperty('--border-subtle', hslToRgb(src.borderSubtle.h, src.borderSubtle.s, src.borderSubtle.l));
    root.setProperty('--text-1', hslToRgb(src.text1.h, src.text1.s, src.text1.l));
    root.setProperty('--text-2', hslToRgb(src.text2.h, src.text2.s, src.text2.l));
    root.setProperty('--text-3', hslToRgb(src.text3.h, src.text3.s, src.text3.l));
    root.setProperty('--text-on-accent', hslToRgb(src.textOnAccent.h, src.textOnAccent.s, src.textOnAccent.l));
    root.setProperty('--accent', hslToRgb(src.accent.h, src.accent.s, src.accent.l));
    root.setProperty('--accent-2', hslToRgb(src.accent2.h, src.accent2.s, src.accent2.l));
    root.setProperty('--accent-soft', hslToRgba(src.accentSoft.h, src.accentSoft.s, src.accentSoft.l, src.accentSoft.a));
    root.setProperty('--accent-purple', hslToRgb(src.accentPurple.h, src.accentPurple.s, src.accentPurple.l));
    root.setProperty('--accent-purple-soft', hslToRgba(src.accentPurpleSoft.h, src.accentPurpleSoft.s, src.accentPurpleSoft.l, src.accentPurpleSoft.a));
    root.setProperty('--success', hslToRgb(src.success.h, src.success.s, src.success.l));
    root.setProperty('--success-soft', hslToRgba(src.successSoft.h, src.successSoft.s, src.successSoft.l, src.successSoft.a));
    root.setProperty('--warning', hslToRgb(src.warning.h, src.warning.s, src.warning.l));
    root.setProperty('--warning-soft', hslToRgba(src.warningSoft.h, src.warningSoft.s, src.warningSoft.l, src.warningSoft.a));
    root.setProperty('--danger', hslToRgb(src.danger.h, src.danger.s, src.danger.l));
    root.setProperty('--danger-soft', hslToRgba(src.dangerSoft.h, src.dangerSoft.s, src.dangerSoft.l, src.dangerSoft.a));
    root.setProperty('--info', hslToRgb(src.info.h, src.info.s, src.info.l));
    root.setProperty('--info-soft', hslToRgba(src.infoSoft.h, src.infoSoft.s, src.infoSoft.l, src.infoSoft.a));

    root.setProperty('--shadow-1', this.targetLight ? '0 1px 2px rgba(26, 32, 44, 0.05), 0 1px 3px rgba(26, 32, 44, 0.04)' : '0 1px 2px rgba(0, 0, 0, 0.5), 0 1px 4px rgba(0, 0, 0, 0.3)');
    root.setProperty('--shadow-2', this.targetLight ? '0 4px 16px rgba(26, 32, 44, 0.08), 0 2px 6px rgba(26, 32, 44, 0.05)' : '0 4px 16px rgba(0, 0, 0, 0.5), 0 2px 6px rgba(0, 0, 0, 0.35)');
    root.setProperty('--shadow-glow', this.targetLight ? '0 0 24px rgba(45, 95, 211, 0.15)' : '0 0 28px rgba(91, 141, 239, 0.18)');
    root.setProperty('--gradient-primary', this.targetLight ? 'linear-gradient(135deg, #2d5fd3 0%, #6d3fc4 100%)' : 'linear-gradient(135deg, #5b8def 0%, #8b6fef 100%)');
  }

  toggle() { this.setLightTheme(!this.targetLight); }

  _getDarkPalette() {
    if (!this._darkPalette) this._darkPalette = generatePalette(false);
    return this._darkPalette;
  }

  _getLightPalette() {
    if (!this._lightPalette) this._lightPalette = generatePalette(true);
    return this._lightPalette;
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
    } catch (e) {
    }
  }

  _restorePreference() {
    try {
      const saved = localStorage.getItem('llamaui-theme');
      if (saved !== null) {
        this._doSetLightTheme(saved === 'light');
      }
    } catch (e) {
    }
  }

  isLightTheme() {
    return this.targetLight;
  }

  destroy() {
    if (this._debounceTimer) {
      clearTimeout(this._debounceTimer);
      this._debounceTimer = null;
    }
  }
}

const themeManager = new ThemeManager();

window.themeManager = themeManager;
