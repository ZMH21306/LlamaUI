#!/usr/bin/env python3
"""
依据约定的 10 大板块模板增量更新 README.md。
仅修改与本次变更相关的章节，其他内容保持原样。
"""

import re, sys, subprocess
from pathlib import Path

README = Path("README.md")
if not README.exists():
    print("❌ README.md 不存在，已创建空文件。")
    README.touch()

content = README.read_text(encoding="utf-8")
commit_msg = sys.argv[1] if len(sys.argv) > 1 else ""

def update_about_project():
    if not commit_msg.startswith("feat"):
        return
    desc = commit_msg.split(":", 1)[1].strip()
    bullet = f"- ✅ {desc}"
    pattern = r"(## 关于项目[\s\S]*?### 主要功能\s*\n)(.*?)(\n## |\Z)"
    m = re.search(pattern, content, flags=re.S)
    if not m:
        return
    header, body, tail = m.groups()
    if bullet in body:
        return
    new_body = body.rstrip() + "\n" + bullet + "\n"
    global content
    content = header + new_body + tail

def update_development_roadmap():
    if commit_msg.startswith("feat"):
        desc = commit_msg.split(":", 1)[1].strip()
        line = f"- ✅ {desc}"
        section = "✅ 已完成功能"
    elif commit_msg.startswith("fix"):
        desc = commit_msg.split(":", 1)[1].strip()
        line = f"- {desc} - 待修复"
        section = "🐛 已知问题"
    else:
        return
    pattern = rf"(## 开发路线[\s\S]*?{section}\s*\n)(.*?)(\n## |\Z)"
    m = re.search(pattern, content, flags=re.S)
    if not m:
        return
    header, body, tail = m.groups()
    if line in body:
        return
    new_body = body.rstrip() + "\n" + line + "\n"
    global content
    content = header + new_body + tail

def update_release_section():
    if not commit_msg.startswith("release"):
        return
    try:
        version = subprocess.check_output("git describe --tags --abbrev=0", shell=True, text=True).strip()
    except subprocess.CalledProcessError:
        version = "v0.0.0"
    content = re.sub(r"(v\d+\.\d+\.\d+)", version, content)

if __name__ == "__main__":
    update_about_project()
    update_development_roadmap()
    update_release_section()
    README.write_text(content, encoding="utf-8")
    print("✅ README.md 已更新")
