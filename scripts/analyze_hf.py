with open('d:/github/LlamaUI/dist/hf-store.js','r',encoding='utf-8',errors='replace') as f:
    content = f.read()

idx = content.find("hf-download-progress")
if idx >= 0:
    print(content[max(0,idx-200):idx+900])