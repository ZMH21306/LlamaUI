import os

# ---- 检测模块（detect 4 阶段优先级链） ----
with open('src/detect/stage3.rs','r',encoding='utf-8',errors='replace') as f:
    s3 = f.read()
checks_detect_s3 = [
    ('home.join(".llamaui").join("llama-cpp")', 'key_dir_roots 含 LlamaUI 默认安装目录'),
    ('home.join(".llamaui").join("llama-cpp").join("models")', 'key_dirs_models 含 LlamaUI 默认模型目录'),
]
for pat, desc in checks_detect_s3:
    print(('PASS' if pat in s3 else 'FAIL'), desc)

with open('src/detect/stage4.rs','r',encoding='utf-8',errors='replace') as f:
    s4 = f.read()
checks_detect_s4 = [
    ('fn full_disk_llama', 'full_disk_llama 存在'),
    ('fn full_disk_models', 'full_disk_models 存在'),
    ('dirs::home_dir()', 'home 目录单独扫描（不被 users 黑名单拦截）'),
    ('find_exe_recursive(&home', 'llama 全盘扫 home'),
    ('find_gguf_dir_recursive(&home', 'models 全盘扫 home'),
]
for pat, desc in checks_detect_s4:
    print(('PASS' if pat in s4 else 'FAIL'), desc)

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
    ('safe_filename', 'P0 修复：download_id 用 basename（safe_filename）构造'),
    ('fn cancel_hf_download', 'cancel_hf_download command 存在'),
    ('let base = filename.rsplit', 'cancel_hf_download 用 basename 构造 download_id'),
    ('const HF_API_BASE', '官方 API 地址常量（不再有镜像）'),
    ('const HF_RESOLVE_BASE', '官方下载地址常量（不再有镜像）'),
    ('pub async fn search_hf_models(state: State<\'_, HfState>, query: String, limit: Option<usize>)', 'search_hf_models 无 offset 参数'),
]
for pat, desc in checks:
    print(('PASS' if pat in rust else 'FAIL'), desc)
# Negative checks: HfSource enum and mirror code must be gone
for pat, desc in [
    ('pub enum HfSource', 'HfSource enum 已删除'),
    ('set_hf_source', 'set_hf_source command 已删除'),
    ('get_hf_source', 'get_hf_source command 已删除'),
    ('load_hf_source', 'load_hf_source 持久化读取已删除'),
    ('save_hf_source', 'save_hf_source 持久化写入已删除'),
    ('hf-mirror', 'hf-mirror 镜像引用已删除'),
    ('hf_source_path', 'hf_source_path 持久化路径已删除'),
    ('offset: Option<usize>', 'offset 分页参数已删除'),
]:
    print(('PASS' if pat not in rust else 'FAIL'), desc)

# Check llama_downloader
with open('src/download/llama_downloader.rs','r',encoding='utf-8',errors='replace') as f:
    ld = f.read()
checks2 = [
    ('std::thread::scope', 'parallel HEAD requests'),
    ('let urls: Vec<String> = candidates', 'clone urls before threads'),
    ('s.spawn(move || (i, curl_head(&url_owned)))', 'spawn curl_head in thread'),
    ('for (i, (_, result)) in results.iter().enumerate()', 'iterate results correctly'),
    ('pub const FINDING_ASSET_START: f64 = 0.08;', 'new stage_progress ratios'),
    ('pub const DOWNLOAD_START: f64 = 0.22;', 'download starts at 22%'),
    ('pub const DOWNLOAD_END: f64 = 0.80;', 'download ends at 80%'),
    ('cancel_token: Option<&std::sync::atomic::AtomicBool>,', 'cancel_token parameter'),
    ('pub speed_mbps: f64,', 'speed_mbps in DownloadProgress'),
    ('pub eta_secs: Option<u64>,', 'eta_secs in DownloadProgress'),
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
    ('filename:it.fn', 'cancel uses full filename (base extracted in backend)'),
    ('data-fsize=', 'pass file size to button'),
    ('startD(fmid,fpath,fsize)', 'pass size to startD'),
]
for pat, desc in checks3:
    print(('PASS' if pat in js else 'FAIL'), desc)
# Negative checks: pagination code must be gone from frontend
for pat, desc in [
    ('async function loadMore', 'loadMore 函数已删除'),
    ('_offset=0, _hasMore=false', 'pagination state variables 已删除'),
    ('hfLoadMoreBtn', 'load-more button element 已删除'),
    ('loadMore)', 'loadMore button click listener registered in init 已删除'),
    ('offset:_offset', 'search passes current offset 已删除'),
    ('offset:_offset', 'loadMore passes current offset 已删除'),
]:
    print(('PASS' if pat not in js else 'FAIL'), desc)
# Negative checks: all mirror/source code must be gone from frontend
for pat, desc in [
    ('get_hf_source', 'get_hf_source 调用已删除'),
    ('set_hf_source', 'set_hf_source 调用已删除'),
    ('hfSourceSelect', 'hfSourceSelect 元素引用已删除'),
    ('hf-mirror', 'hf-mirror 镜像引用已删除'),
    ('hf-source-changed', 'dead hf-source-changed event listener removed'),
]:
    print(('PASS' if pat not in js else 'FAIL'), desc)

with open('dist/hf-store.html','r',encoding='utf-8',errors='replace') as f:
    html = f.read()
for pat, desc in [
    ('hfSourceSelect', 'hfSourceSelect 元素已删除'),
    ('hf-mirror', 'hf-mirror 镜像引用已删除'),
    ('hfLoadMoreBtn', 'hfLoadMoreBtn 按钮已删除'),
]:
    print(('PASS' if pat not in html else 'FAIL'), desc)

with open('dist/main.js','r',encoding='utf-8',errors='replace') as f:
    js2 = f.read()
checks_js = [
    ("formatSpeed(mbps)", 'formatSpeed helper'),
    ("formatETA(secs)", 'formatETA helper'),
    ("setBtnProgress", 'text progress helper'),
    ("setBtnComplete", 'text complete helper'),
    ("setBtnReset", 'text reset helper'),
]
for pat, desc in checks_js:
    print(('PASS' if pat in js2 else 'FAIL'), desc)

with open('dist/styles.css','r',encoding='utf-8',errors='replace') as f:
    css = f.read()
checks_css = []
for pat, desc in checks_css:
    print(('PASS' if pat in css else 'FAIL'), desc)
# Negative checks: load-more button CSS must be gone
for pat, desc in [
    ('.hf-load-more-btn', 'hf-load-more-btn CSS rule 已删除'),
]:
    print(('PASS' if pat not in css else 'FAIL'), desc)

