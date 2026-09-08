import re

path = 'd:/github/LlamaUI/dist/styles.css'

with open(path, 'r', encoding='utf-8', errors='replace') as f:
    content = f.read()

# New :root block (dark theme)
new_root = """:root {
  /* =========================================================
     Dark theme - Obsidian Abyss
     Deep blue-black surfaces with refined indigo-blue accent
     ========================================================= */
  --bg-0: #08090e;
  --bg-1: #0d0f16;
  --bg-2: #12151d;
  --bg-3: #181c26;
  --bg-input: #0f1218;
  --bg-elevated: #151922;
  --border: #1e2433;
  --border-strong: #2a3145;
  --border-subtle: #161a24;
  --text-1: #e2e6ef;
  --text-2: #9aa3b8;
  --text-3: #5f6a7e;
  --text-on-accent: #ffffff;
  --accent: #5b8def;
  --accent-2: #7c9fff;
  --accent-soft: rgba(91, 141, 239, 0.14);
  --accent-purple: #8b6fef;
  --accent-purple-soft: rgba(139, 111, 239, 0.14);
  --success: #4ade80;
  --success-soft: rgba(74, 222, 128, 0.14);
  --warning: #fbbf24;
  --warning-soft: rgba(251, 191, 36, 0.14);
  --danger: #f87171;
  --danger-soft: rgba(248, 113, 113, 0.14);
  --info: #60a5fa;
  --info-soft: rgba(96, 165, 250, 0.14);
  --gradient-primary: linear-gradient(135deg, #5b8def 0%, #8b6fef 100%);
  --gradient-accent: linear-gradient(135deg, var(--accent), var(--accent-purple));
  --s-1: 4px;
  --s-2: 6px;
  --s-3: 8px;
  --s-4: 12px;
  --s-5: 16px;
  --s-6: 20px;
  --s-7: 24px;
  --r-sm: 6px;
  --r-md: 10px;
  --r-lg: 14px;
  --radius-md: var(--r-md);
  --shadow-1: 0 1px 2px rgba(0, 0, 0, 0.5), 0 1px 4px rgba(0, 0, 0, 0.3);
  --shadow-2: 0 4px 16px rgba(0, 0, 0, 0.5), 0 2px 6px rgba(0, 0, 0, 0.35);
  --shadow-glow: 0 0 28px rgba(91, 141, 239, 0.18);
  --border-1: var(--border);
  --mono: ui-monospace, "Cascadia Code", "JetBrains Mono", Menlo, Consolas, monospace;
  --fs-xs: 11px;
  --fs-sm: 12px;
  --fs-base: 13px;
  --fs-md: 14px;
  --fs-lg: 16px;
  --fs-xl: 20px;
  --ease: cubic-bezier(0.25, 0.46, 0.45, 0.94);
  --ease-out: cubic-bezier(0.215, 0.61, 0.355, 1);
  --dur: 250ms;
  --dur-fast: 150ms;
  --dur-slow: 350ms;
  --topbar-h: 56px;
  --pane-left-w: 420px;
  --pane-right-w: 280px;
}"""

# Replace :root block
pattern_root = r':root\s*\{[^}]*\}'
content = re.sub(pattern_root, new_root, content, count=1)

# New body.light-theme block
new_light = """body.light-theme {
  /* =========================================================
     Light theme - Alabaster Studio
     Warm white surfaces with deep navy-blue accent
     ========================================================= */
  --bg-0: #f3f4f7;
  --bg-1: #ffffff;
  --bg-2: #f8f9fc;
  --bg-3: #eef1f6;
  --bg-input: #ffffff;
  --bg-elevated: #ffffff;
  --border: #d4d9e2;
  --border-strong: #b0bac8;
  --border-subtle: #e8ecf2;
  --text-1: #1a202c;
  --text-2: #4a5568;
  --text-3: #8a94a6;
  --text-on-accent: #ffffff;
  --accent: #2d5fd3;
  --accent-2: #4a7fff;
  --accent-soft: rgba(45, 95, 211, 0.12);
  --accent-purple: #6d3fc4;
  --accent-purple-soft: rgba(109, 63, 196, 0.12);
  --success: #1a8a55;
  --success-soft: rgba(26, 138, 85, 0.12);
  --warning: #b45309;
  --warning-soft: rgba(180, 83, 9, 0.12);
  --danger: #c62828;
  --danger-soft: rgba(198, 40, 40, 0.12);
  --info: #1d4ed8;
  --info-soft: rgba(29, 78, 216, 0.12);
  --gradient-primary: linear-gradient(135deg, #2d5fd3 0%, #6d3fc4 100%);
  --gradient-accent: linear-gradient(135deg, var(--accent), var(--accent-purple));
  --shadow-1: 0 1px 2px rgba(26, 32, 44, 0.05), 0 1px 3px rgba(26, 32, 44, 0.04);
  --shadow-2: 0 4px 16px rgba(26, 32, 44, 0.08), 0 2px 6px rgba(26, 32, 44, 0.05);
  --shadow-glow: 0 0 24px rgba(45, 95, 211, 0.15);
}"""

# Replace body.light-theme block
pattern_light = r'body\.light-theme\s*\{[^}]*\}'
content = re.sub(pattern_light, new_light, content, count=1)

with open(path, 'w', encoding='utf-8') as f:
    f.write(content)

print('styles.css updated successfully')
