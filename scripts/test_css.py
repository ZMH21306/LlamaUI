
import re

path = 'd:/github/LlamaUI/dist/styles.css'
with open(path, 'r', encoding='utf-8', errors='replace') as f:
    content = f.read()

# Find :root block
root_match = re.search(r':root\s*\{', content)
print('root match at:', root_match.start() if root_match else 'NOT FOUND')
if root_match:
    # Find matching closing brace
    start = root_match.end()
    brace_count = 1
    i = start
    while i < len(content) and brace_count > 0:
        if content[i] == '{':
            brace_count += 1
        elif content[i] == '}':
            brace_count -= 1
        i += 1
    root_end = i
    print('root block:', repr(content[root_match.start():root_end][:100]))
    print('root block length:', root_end - root_match.start())
