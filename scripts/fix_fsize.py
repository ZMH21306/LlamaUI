import sys

with open('dist/hf-store.js','rb') as f:
    data = f.read()

q = chr(34)  # double quote
s = chr(39)  # single quote
bs = chr(92)  # backslash
xc = bytes([0xe4,0xb8,0x8b,0xe8,0xbd,0xbd])  # 下载 in UTF-8

old = b'data-fmid=\x22\x5c\x27+mid+\x5c\x27\x22>' + xc
new = b' data-fsize=\x22\x5c\x27+sz+\x5c\x27\x22 data-fmid=\x22\x5c\x27+mid+\x5c\x27\x22>' + xc
new = b'data-fsize=' + q.encode() + bs.encode() + s.encode() + b'+sz+' + bs.encode() + s.encode() + q.encode() + b' data-fmid=' + q.encode() + bs.encode() + s.encode() + b'+mid+' + bs.encode() + s.encode() + q.encode() + b'>' + xc

print('found:', old in data)
if old in data:
    data = data.replace(old, new)
    with open('dist/hf-store.js','wb') as f:
        f.write(data)
    print('OK: replaced')
else:
    idx = data.find(b'data-fmid')
    if idx >= 0:
        print('context:', data[idx-30:idx+60])