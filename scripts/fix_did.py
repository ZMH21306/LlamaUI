with open('src/commands/hf_model_cmd.rs','r',encoding='utf-8',errors='replace') as f:
    content = f.read()

# Fix 1: download_id construction in download_hf_model
old1 = 'let download_id = format!("{}::{}", model_id, safe_filename);'
new1 = 'let download_id = format!("{}::{}", model_id, filename);'
print('found1:', old1 in content)
if old1 in content:
    content = content.replace(old1, new1)
    print('OK: replaced download_id construction')

# Fix 2: download_id in complete event
old2 = 'download_id: format!("{}::{}", model_id, safe_filename),'
new2 = 'download_id: format!("{}::{}", model_id, filename),'
print('found2:', old2 in content)
if old2 in content:
    content = content.replace(old2, new2)
    print('OK: replaced complete event download_id')

with open('src/commands/hf_model_cmd.rs','w',encoding='utf-8') as f:
    f.write(content)
print('File saved')