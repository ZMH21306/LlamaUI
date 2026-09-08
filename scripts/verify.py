import os

# Check Rust source
with open('src/commands/hf_model_cmd.rs','r',encoding='utf-8',errors='replace') as f:
    rust = f.read()
checks = [
    ('expected_size: Option<u64>', 'download_hf_model signature'),
    ('.or(expected_size)', 'fallback to expected_size'),
    ('pub download_id: String', 'download_id in HfDownloadProgress'),
    ('download_id: download_id.clone()', 'download_id in init emit'),
    ('download_id: download_id_for_emit.clone()', 'download_id in downloading emit'),
    ('download_id: format!', 'download_id in complete emit'),
]
for pat, desc in checks:
    print(('PASS' if pat in rust else 'FAIL'), desc)

# Check llama_downloader
with open('src/llama_downloader.rs','r',encoding='utf-8',errors='replace') as f:
    ld = f.read()
checks2 = [
    ('std::thread::scope', 'parallel HEAD requests'),
    ('let urls: Vec<String> = candidates.iter().map(|a| a.browser_download_url.clone()).collect()', 'clone urls before threads'),
    ('s.spawn(move || (i, curl_head(&url_owned)))', 'spawn curl_head in thread'),
    ('for (i, (_, result)) in results.iter().enumerate()', 'iterate results correctly'),
]
for pat, desc in checks2:
    print(('PASS' if pat in ld else 'FAIL'), desc)

# Check frontend
with open('dist/hf-store.js','r',encoding='utf-8',errors='replace') as f:
    js = f.read()
checks3 = [
    ('expected_size:sz||0', 'pass expected_size to invoke'),
    ('did: p.download_id', 'match by download_id'),
    ('if(_queue.find(function(d){return d.id===dl;}))return;', 'prevent phantom re-creation'),
    ('filename:it.fn', 'cancel uses full filename'),
    ('data-fsize=', 'pass file size to button'),
    ('startD(fmid,fpath,fsize)', 'pass size to startD'),
]
for pat, desc in checks3:
    print(('PASS' if pat in js else 'FAIL'), desc)

with open('dist/styles.css','r',encoding='utf-8',errors='replace') as f:
    css = f.read()
checks4 = [
    ('width: 30% !important', 'old indeterminate width removed'),
    ('.download-bar-fill.indeterminate::after', 'pseudo-element for indeterminate'),
]
for pat, desc in checks4:
    print(('PASS' if pat not in css else 'FAIL'), desc)