#!/usr/bin/env python3
"""
从 git diff 自动生成符合 Conventional Commits 规范的提交信息。
- feat: 新功能
- fix : Bug 修复
- docs: 文档改动
- refactor: 重构
- chore: 其他杂务
"""

import subprocess, sys, re

def git_diff_files():
    out = subprocess.check_output("git diff --cached --name-only", shell=True, text=True)
    return [p.strip() for p in out.splitlines() if p]

def infer_type(files):
    for f in files:
        if f.endswith(('.py', '.js', '.ts', '.tsx', '.rs', '.cpp', '.c', '.java')):
            return "feat"
        if f.endswith(('.md', '.txt', '.rst')):
            return "docs"
    return "chore"

def main():
    files = git_diff_files()
    if not files:
        print("chore: No changes")
        sys.exit(0)
    ctype = infer_type(files)
    diff_snippet = subprocess.check_output("git diff --cached -U0 | head -n 5", shell=True, text=True)
    summary = re.sub(r"\s+", " ", diff_snippet).strip()
    summary = re.sub(r"[^A-Za-z0-9 ,.-]", "", summary)[:60]
    print(f"{ctype}(auto-doc): {summary}")

if __name__ == "__main__":
    main()
