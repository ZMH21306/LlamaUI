# -*- coding: utf-8 -*-
import sys
path = r'd:\\github\\LlamaUI\\src\\config\\store.rs'
with open(path, 'r', encoding='utf-8') as f:
    lines = f.readlines()

# add new fields after custom_command
insert_idx = -1
for i, line in enumerate(lines):
    if 'pub custom_command: String,' in line:
        insert_idx = i
        break

if insert_idx != -1:
    new_lines = [
        '  // ===== new fields: UI state and HF config =====\n',
        '  /// whether light theme (false = dark, true = light)\n',
        '  pub light_theme: bool,\n',
        '  /// left panel width (px)\n',
        '  pub pane_left_w: u16,\n',
        '  /// right panel width (px)\n',
        '  pub pane_right_w: u16,\n',
        '  /// window width (px)\n',
        '  pub window_width: u16,\n',
        '  /// window height (px)\n',
        '  pub window_height: u16,\n',
        '  /// HF Token (encrypted storage, temporarily not encrypted)\n',
        '  pub hf_token_encrypted: Option<String>,\n',
        '  /// last accessed remote server URL\n',
        '  pub last_remote_url: Option<String>,\n'
    ]
    
    for i, line in enumerate(new_lines):
        lines.insert(insert_idx + 1 + i, line)

# update Default implementation
for i in range(len(lines)):
    if 'impl Default for AppConfig' in lines[i]:
        for j in range(i, len(lines)):
            if 'Self {' in lines[j]:
                for k in range(j, len(lines)):
                    if 'custom_command: DEFAULT_PRO_CUSTOM_COMMAND.to_string(),' in lines[k]:
                        lines.insert(k + 1, '             light_theme: false,\n')
                        lines.insert(k + 2, '             pane_left_w: 420,\n')
                        lines.insert(k + 3, '             pane_right_w: 280,\n')
                        lines.insert(k + 4, '             window_width: 1400,\n')
                        lines.insert(k + 5, '             window_height: 900,\n')
                        lines.insert(k + 6, '             hf_token_encrypted: None,\n')
                        lines.insert(k + 7, '             last_remote_url: None,\n')
                        break
                break
        break

# update version
for i in range(len(lines)):
    if 'const CURRENT_CONFIG_VERSION: u32' in lines[i]:
        lines[i] = 'const CURRENT_CONFIG_VERSION: u32 = 2;\n'
        break

with open(path, 'w', encoding='utf-8') as f:
    f.writelines(lines)
print('Updated AppConfig struct')
