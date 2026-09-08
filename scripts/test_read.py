
import os

path = 'd:/github/LlamaUI/dist/styles.css'
with open(path, 'r', encoding='utf-8') as f:
    content = f.read()

# Replace :root block (dark theme)
old_root_start = content.find(':root {')
old_root_end = content.find('}', old_root_start) + 1
old_root = content[old_root_start:old_root_end]
print('Found :root block from', old_root_start, 'to', old_root_end)
print('Old :root first 100 chars:', repr(old_root[:100]))
