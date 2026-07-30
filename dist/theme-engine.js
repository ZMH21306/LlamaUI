// ============================================================
// LlamaUI 主题引擎 v2
// 基于 HSL/Oklab 色彩空间的丝滑主题切换系统
// 核心原理：
//   1. 定义基础色板（HSL），通过调整 L/S 生成衍生色
//   2. 切换时在 Oklab 空间中进行插值，避免发灰发脏
//   3. 使用三次贝塞尔缓动函数控制动画进度
//   4. 监听系统主题变化，带 300ms 防抖
// ============================================================
'use strict';

// ============= 基础色板定义（HSL） =============
// 色相 H：主色调固定为 225（蓝紫色系）
// 饱和度 S：暗色模式提高饱和度，浅色模式降低饱和度
// 明度 L：暗色模式降低明度，浅色模式提高明度

const BASE_PALETTE = {
  // 主色相（蓝紫系）
  primaryH: 225,
  // 状态色色相
  successH: 150,
  warningH: 37,
  dangerH: 0,
  infoH: 217,
};

/**
 * 根据主题模式从基础色板生成完整色板
 * @param {boolean} isLight - 是否为浅色模式
 * @returns {Object} 生成的色板（HSL 格式）
 */
function generatePalette(isLight) {
  const { primaryH, successH, warningH, dangerH, infoH } = BASE_PALETTE;

  if (isLight) {
    // 浅色模式：高明度、低饱和度
    return {
      // 背景色阶（从最深到最浅）
      bg0: { h: primaryH, s: 15, l: 96 },    // --bg-0: 最外层背景
      bg1: { h: primaryH, s: 12, l: 100 },   // --bg-1: 卡片背景
      bg2: { h: primaryH, s: 10, l: 94 },    // --bg-2: 输入框/按钮
      bg3: { h: primaryH, s: 8,  l: 91 },    // --bg-3: hover 状态
      bgInput: { h: primaryH, s: 12, l: 100 },
      bgElevated: { h: primaryH, s: 12, l: 100 },

      // 边框
      border: { h: primaryH, s: 15, l: 88 },
      borderStrong: { h: primaryH, s: 14, l: 82 },
      borderSubtle: { h: primaryH, s: 10, l: 94 },

      // 文本（根据 WCAG 2.1，浅色背景上用深色文本）
      text1: { h: primaryH, s: 20, l: 12 },  // 主文本
      text2: { h: primaryH, s: 14, l: 35 },  // 次要文本
      text3: { h: primaryH, s: 10, l: 58 },  // 弱化文本
      textOnAccent: { h: 0, s: 0, l: 100 },  // 强调色上的文本（白色）

      // 强调色
      accent: { h: primaryH, s: 100, l: 55 },
      accent2: { h: primaryH, s: 100, l: 65 },
      accentSoft: { h: primaryH, s: 100, l: 55, a: 0.12 },

      // 状态色
      success: { h: successH, s: 50, l: 35 },
      successSoft: { h: successH, s: 50, l: 35, a: 0.15 },
      warning: { h: warningH, s: 80, l: 50 },
      warningSoft: { h: warningH, s: 80, l: 50, a: 0.16 },
      danger: { h: dangerH, s: 80, l: 55 },
      dangerSoft: { h: dangerH, s: 80, l: 55, a: 0.16 },
      info: { h: infoH, s: 90, l: 60 },
      infoSoft: { h: infoH, s: 90, l: 60, a: 0.15 },
    };
  } else {
    // 暗色模式：低明度、高饱和度
    return {
      bg0: { h: primaryH, s: 20, l: 4 },     // --bg-0: 最外层背景（最深）
      bg1: { h: primaryH, s: 16, l: 7 },     // --bg-1: 卡片背景
      bg2: { h: primaryH, s: 14, l: 10 },    // --bg-2: 输入框/按钮
      bg3: { h: primaryH, s: 12, l: 14 },    // --bg-3: hover 状态
      bgInput: { h: primaryH, s: 16, l: 7 },
      bgElevated: { h: primaryH, s: 18, l: 11 },

      // 边框
      border: { h: primaryH, s: 16, l: 17 },
      borderStrong: { h: primaryH, s: 16, l: 22 },
      borderSubtle: { h: primaryH, s: 14, l: 12 },

      // 文本（暗色背景上用浅色文本）
      text1: { h: primaryH, s: 12, l: 92 },  // 主文本
      text2: { h: primaryH, s: 10, l: 68 },  // 次要文本
      text3: { h: primaryH, s: 8,  l: 48 },  // 弱化文本
      textOnAccent: { h: 0, s: 0, l: 100 },  // 强调色上的文本（白色）

      // 强调色（暗色模式下更鲜艳）
      accent: { h: primaryH, s: 85, l: 62 },
      accent2: { h: primaryH, s: 85, l: 72 },
      accentSoft: { h: primaryH, s: 85, l: 62, a: 0.16 },

      // 状态色
      success: { h: successH, s: 70, l: 60 },
      successSoft: { h: successH, s: 70, l: 60, a: 0.15 },
      warning: { h: warningH, s: 90, l: 55 },
      warningSoft: { h: warningH, s: 90, l: 55, a: 0.16 },
      danger: { h: dangerH, s: 90, l: 62 },
      dangerSoft: { h: dangerH, s: 90, l: 62, a: 0.16 },
      info: { h: infoH, s: 90, l: 68 },
      infoSoft: { h: infoH, s: 90, l: 68, a: 0.15 },
    };
  }
}

// ============= Oklab 色彩空间转换 =============
// 为什么使用 Oklab？
// Oklab 是感知均匀的色彩空间，意味着在这个空间中两点之间的
// 欧氏距离与人眼感知到的颜色差异成正比。
// 相比之下，RGB 空间不是感知均匀的——在 RGB 中线性插值
// 会导致颜色过渡发灰、发脏，因为 RGB 的线性路径在感知上
// 并不线性。Oklab 的 lerp 能保证过渡自然、鲜艳。

/**
 * 将 HSL 转换为 RGB（0-1 范围）
 */
function hslToRgb(h, s, l) {
  h = ((h % 360) + 360) % 360 / 360;
  s = Math.max(0, Math.min(1, s / 100));
  l = Math.max(0, Math.min(1, l / 100));

  if (s === 0) {
    return { r: l, g: l, b: l };
  }

  const hue2rgb = (p, q, t) => {
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1/6) return p + (q - p) * 6 * t;
    if (t < 1/2) return q;
    if (t < 2/3) return p + (q - p) * (2/3 - t) * 6;
    return p;
  };

  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;

  return {
    r: hue2rgb(p, q, h + 1/3),
    g: hue2rgb(p, q, h),
    b: hue2rgb(p, q, h - 1/3),
  };
}

/**
 * 将 RGB 转换为 Oklab
 * 参考：https://bottosson.github.io/posts/oklab/
 */
function rgbToOklab(r, g, b) {
  // RGB -> linear RGB
  const linearize = (c) => c >= 0 ? Math.pow(c, 2.4) : -Math.pow(-c, 2.4);
  const rl = linearize(r);
  const gl = linearize(g);
  const bl = linearize(b);

  // linear RGB -> LMS (using Oklab's matrix)
  const l = 0.4122214708 * rl + 0.5363325363 * gl + 0.0514459929 * bl;
  const m = 0.2119034982 * rl + 0.6806995451 * gl + 0.1073969566 * bl;
  const s = 0.0883024619 * rl + 0.2817188376 * gl + 0.6299787005 * bl;

  // cube root
  const cbrt = (x) => Math.sign(x) * Math.pow(Math.abs(x), 1/3);
  const ll = cbrt(l);
  const ml = cbrt(m);
  const sl = cbrt(s);

  // LMS -> Oklab
  return {
    L: 0.2104542553 * ll + 0.7936177850 * ml - 0.0040720468 * sl,
    a: 1.9779984951 * ll - 2.4285922050 * ml + 0.4505937099 * sl,
    b: 0.0259040371 * ll + 0.7827717662 * ml - 0.8086757660 * sl,
  };
}

/**
 * 将 Oklab 转换为 RGB
 */
function oklabToRgb(ok) {
  const { L, a, b } = ok;

  // Oklab -> LMS'
  const ll = L + 0.3963377774 * a + 0.2158037573 * b;
  const ml = L - 0.1055613458 * a - 0.0638541728 * b;
  const sl = L - 0.0894841775 * a - 1.2914855480 * b;

  // cube
  const l = ll * ll * ll;
  const m = ml * ml * ml;
  const s = sl * sl * sl;

  // LMS -> linear RGB
  const rl = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
  const gl = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
  const bl = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

  // linearize inverse (gamma correction)
  const delinearize = (c) => c >= 0 ? Math.pow(c, 1/2.4) : -Math.pow(-c, 1/2.4);

  return {
    r: Math.max(0, Math.min(1, delinearize(rl))),
    g: Math.max(0, Math.min(1, delinearize(gl))),
    b: Math.max(0, Math.min(1, delinearize(bl))),
  };
}

/**
 * 在 Oklab 空间中对两个 HSL 颜色进行插值
 * 为什么在这里插值？
 * 直接在 RGB 空间插值会走"捷径"——比如从深蓝到浅黄，
 * RGB 会经过灰色区域，因为 RGB 的线性路径在感知上是非线性的。
 * 在 Oklab 空间中，L（明度）、a（绿-红轴）、b（蓝-黄轴）
 * 三个通道独立变化，插值路径在视觉上均匀，颜色过渡自然鲜艳。
 *
 * @param {Object} from - 起始 HSL 颜色
 * @param {Object} to - 目标 HSL 颜色
 * @param {number} t - 插值因子 (0-1)
 * @returns {string} CSS rgb/rgba 字符串
 */
function lerpHslInOklab(from, to, t) {
  // HSL -> RGB -> Oklab
  const fromRgb = hslToRgb(from.h, from.s, from.l);
  const toRgb = hslToRgb(to.h, to.s, to.l);

  const fromLab = rgbToOklab(fromRgb.r, fromRgb.g, fromRgb.b);
  const toLab = rgbToOklab(toRgb.r, toRgb.g, toRgb.b);

  // 在 Oklab 空间中线性插值
  const lerpedLab = {
    L: fromLab.L + (toLab.L - fromLab.L) * t,
    a: fromLab.a + (toLab.a - fromLab.a) * t,
    b: fromLab.b + (toLab.b - fromLab.b) * t,
  };

  // Oklab -> RGB -> CSS 字符串
  const lerpedRgb = oklabToRgb(lerpedLab);
  const r = Math.round(lerpedRgb.r * 255);
  const g = Math.round(lerpedRgb.g * 255);
  const b = Math.round(lerpedRgb.b * 255);

  const alpha = from.a !== undefined || to.a !== undefined
    ? (from.a || 1) + ((to.a || 1) - (from.a || 1)) * t
    : 1;

  return alpha < 1
    ? `rgba(${r}, ${g}, ${b}, ${alpha.toFixed(2)})`
    : `rgb(${r}, ${g}, ${b})`;
}

// ============= 动画引擎 =============

/**
 * 三次贝塞尔缓动函数（ease-in-out cubic）
 * 为什么使用缓动函数？
 * 线性插值在视觉上显得机械。缓动函数让动画开始和结束时
 * 速度变慢，中间加速，符合物理世界的运动规律，感觉更自然。
 * 公式：t < 0.5 ? 4t³ : 1 - (-2t+2)³/2
 */
function easeInOutCubic(t) {
  return t < 0.5
    ? 4 * t * t * t
    : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

// ============= ThemeManager =============

const ANIMATION_DURATION = 400; // 动画时长 400ms

class ThemeManager {
  constructor() {
    // 当前主题模式：false = 暗色, true = 亮色
    this.targetLight = false;
    this.currentLight = false;

    // 动画状态
    this.animStartTime = null;
    this.animDuration = ANIMATION_DURATION;
    this.animFrameId = null;

    // 缓存的色板
    this._darkPalette = null;
    this._lightPalette = null;

    // 防抖相关
    this._debounceTimer = null;
    this._debounceMs = 300;
    this._pendingLight = null; // 防抖期间等待设置的目标值

    // CSS 变量名列表（需要动画过渡的）
    this.animatedVars = [
      'bg-0', 'bg-1', 'bg-2', 'bg-3', 'bg-input', 'bg-elevated',
      'border', 'border-strong', 'border-subtle',
      'text-1', 'text-2', 'text-3',
      'accent', 'accent-2',
      'success', 'warning', 'danger', 'info',
    ];

    // 带 alpha 的变量
    this.alphaVars = [
      'accent-soft', 'success-soft', 'warning-soft', 'danger-soft', 'info-soft',
    ];

    // 绑定系统主题监听
    this._bindSystemTheme();

    // 从存储恢复用户偏好
    this._restorePreference();
  }

  /**
   * 获取当前帧的插值颜色值
   * 在动画过程中每帧调用，返回当前过渡进度下的实际颜色
   */
  getCurrentColors() {
    const now = performance.now();
    const elapsed = now - this.animStartTime;
    const rawT = Math.min(1, elapsed / this.animDuration);
    const t = easeInOutCubic(rawT);

    // 更新当前主题状态
    this.currentLight = this.targetLight;

    // 如果动画未完成，安排下一帧
    if (rawT < 1) {
      this.animFrameId = requestAnimationFrame(() => {
        this._applyColors();
      });
    } else {
      this.animFrameId = null;
    }

    return this._computeInterpolatedColors(t);
  }

  /**
   * 计算插值后的颜色值
   */
  _computeInterpolatedColors(t) {
    const dark = this._getDarkPalette();
    const light = this._getLightPalette();
    const colors = {};

    for (const varName of this.animatedVars) {
      const from = dark[varName];
      const to = light[varName];
      if (from && to) {
        colors[`--${varName}`] = lerpHslInOklab(from, to, t);
      }
    }

    for (const varName of this.alphaVars) {
      const from = dark[varName];
      const to = light[varName];
      if (from && to) {
        colors[`--${varName}`] = lerpHslInOklab(from, to, t);
      }
    }

    // 阴影也需要过渡
    colors['--shadow-1'] = t < 0.5
      ? '0 1px 2px rgba(0, 0, 0, 0.4)'
      : `0 1px 3px rgba(0, 0, 0, ${0.1 + (1 - t) * 0.3})`;
    colors['--shadow-2'] = t < 0.5
      ? '0 6px 20px rgba(0, 0, 0, 0.45)'
      : `0 8px 30px rgba(0, 0, 0, ${0.12 + (1 - t) * 0.33})`;

    return colors;
  }

  /**
   * 应用颜色到 CSS 变量
   */
  _applyColors() {
    const colors = this.getCurrentColors();
    for (const [varName, value] of Object.entries(colors)) {
      document.documentElement.style.setProperty(varName, value);
    }
  }

  /**
   * 切换主题（带防抖）
   */
  toggle() {
    this.setLightTheme(!this.targetLight);
  }

  /**
   * 设置主题模式
   * @param {boolean} isLight - 是否为浅色主题
   */
  setLightTheme(isLight) {
    // 如果目标状态相同，忽略
    if (this.targetLight === isLight) {
      return;
    }

    // 记录待处理的目标值（防抖期间可能被多次覆盖）
    this._pendingLight = isLight;

    // 清除之前的防抖定时器
    if (this._debounceTimer) {
      clearTimeout(this._debounceTimer);
    }

    // 设置防抖延迟，300ms 内无新请求才执行
    this._debounceTimer = setTimeout(() => {
      const pending = this._pendingLight;
      this._pendingLight = null;
      if (pending !== null) {
        this._doSetLightTheme(pending);
      }
    }, this._debounceMs);
  }

  /**
   * 实际执行主题切换（无防抖，直接动画）
   */
  _doSetLightTheme(isLight) {
    // 如果目标状态相同且没有在动画，忽略（避免重复触发动画）
    if (this.targetLight === isLight && !this.animFrameId) {
      return;
    }

    this.targetLight = isLight;

    // 启动动画
    this.animStartTime = performance.now();
    if (this.animFrameId) {
      cancelAnimationFrame(this.animFrameId);
    }
    this.animFrameId = requestAnimationFrame(() => {
      this._applyColors();
    });

    // 保存用户偏好
    this._savePreference(isLight);

    // 触发事件
    window.dispatchEvent(new CustomEvent('theme-change', {
      detail: { isLight, progress: 0 }
    }));
  }

  /**
   * 获取暗色色板（懒加载缓存）
   */
  _getDarkPalette() {
    if (!this._darkPalette) {
      this._darkPalette = generatePalette(false);
    }
    return this._darkPalette;
  }

  /**
   * 获取浅色色板（懒加载缓存）
   */
  _getLightPalette() {
    if (!this._lightPalette) {
      this._lightPalette = generatePalette(true);
    }
    return this._lightPalette;
  }

  /**
   * 绑定系统主题变化监听
   * 自动跟随操作系统的深色/浅色模式设置
   */
  _bindSystemTheme() {
    if (window.matchMedia) {
      const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
      const handler = (e) => {
        // 只有当用户没有手动设置过偏好时，才跟随系统
        const userPref = localStorage.getItem('llamui-theme');
        if (userPref === null) {
          this.setLightTheme(!e.matches);
        }
      };

      // 现代浏览器使用 addEventListener
      if (mediaQuery.addEventListener) {
        mediaQuery.addEventListener('change', handler);
      } else if (mediaQuery.addListener) {
        // 兼容旧浏览器
        mediaQuery.addListener(handler);
      }

      // 初始化：如果用户没有设置偏好，跟随系统
      if (localStorage.getItem('llamui-theme') === null) {
        this.setLightTheme(!mediaQuery.matches);
      }
    }
  }

  /**
   * 保存用户偏好到 localStorage
   */
  _savePreference(isLight) {
    try {
      localStorage.setItem('llamui-theme', isLight ? 'light' : 'dark');
    } catch (e) {
      // localStorage 不可用时静默失败
    }
  }

  /**
   * 从 localStorage 恢复用户偏好
   */
  _restorePreference() {
    try {
      const saved = localStorage.getItem('llamui-theme');
      if (saved !== null) {
        this._doSetLightTheme(saved === 'light');
      }
    } catch (e) {
      // localStorage 不可用时使用默认值
    }
  }

  /**
   * 获取当前主题状态
   */
  isLightTheme() {
    return this.targetLight;
  }

  /**
   * 销毁管理器（清理事件监听和动画帧）
   */
  destroy() {
    if (this.animFrameId) {
      cancelAnimationFrame(this.animFrameId);
      this.animFrameId = null;
    }
    if (this._debounceTimer) {
      clearTimeout(this._debounceTimer);
      this._debounceTimer = null;
    }
  }
}

// ============= 导出单例 =============
const themeManager = new ThemeManager();

// 全局暴露（供 main.js 使用）
window.themeManager = themeManager;

// ============= 辅助函数：WCAG 对比度检查 =============
/**
 * 根据背景亮度自动选择文本颜色（黑或白）
 * 使用 WCAG 2.1 相对亮度公式
 * @param {number} r - 红色通道 (0-255)
 * @param {number} g - 绿色通道 (0-255)
 * @param {number} b - 蓝色通道 (0-255)
 * @returns {string} 'black' 或 'white'
 */
function getContrastTextColor(r, g, b) {
  // 将 sRGB 转换为线性 RGB
  const linearize = (c) => {
    c = c / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };

  const rl = linearize(r);
  const gl = linearize(g);
  const bl = linearize(b);

  // 相对亮度
  const luminance = 0.2126 * rl + 0.7152 * gl + 0.0722 * bl;

  // WCAG 2.1: 亮度 > 0.179 时用黑色文本，否则白色
  return luminance > 0.179 ? 'black' : 'white';
}

// 导出辅助函数
window.getContrastTextColor = getContrastTextColor;
