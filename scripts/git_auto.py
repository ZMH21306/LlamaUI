#!/usr/bin/env python3
"""
辅助脚本：完成 git add、commit、pull（rebase）和 push 的完整流程。
如果没有需要提交的改动，会直接退出。
冲突时会提示并结束，返回错误码供上层捕获。
"""
import subprocess, sys, os

def run_cmd(cmd, check=True):
    result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check and result.returncode != 0:
        print(f"❌ 命令执行失败: {cmd}")
        print(result.stderr)
        sys.exit(result.returncode)
    return result.stdout.strip()

def get_current_branch():
    return run_cmd("git branch --show-current")

def main():
    if len(sys.argv) < 2:
        print("用法: python scripts/git_auto.py \"提交信息\"")
        sys.exit(1)
    message = sys.argv[1]
    print("📂 1. 添加所有变更...")
    run_cmd("git add .")
    status = run_cmd("git status --porcelain", check=False)
    if not status:
        print("✅ 没有需要提交的改动，退出。")
        return
    safe_msg = message.replace('"', '\\"')
    print("📝 2. 提交代码...")
    run_cmd(f'git commit -m "{safe_msg}"')
    branch = get_current_branch()
    print(f"🔗 3. 拉取远程分支 origin/{branch} (使用 rebase)...")
    pull = subprocess.run(f"git pull origin {branch} --rebase", shell=True, capture_output=True, text=True)
    if pull.returncode != 0:
        print("❌ 拉取失败，可能有冲突！请手动解决冲突后重新运行脚本。")
        print(pull.stderr)
        sys.exit(1)
    print(f"🚀 4. 推送至远程分支 origin/{branch}...")
    run_cmd(f"git push origin {branch}")
    print("✅ 推送完成！")

if __name__ == "__main__":
    main()
