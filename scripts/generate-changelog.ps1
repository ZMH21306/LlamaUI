# CHANGELOG 鑷姩鐢熸垚鑴氭湰
# 浠庝笂娆?tag 浠ユ潵鐨?commit 鑷姩鎻愬彇鍙樻洿锛屾寜 Conventional Commits 鍒嗙被
# 鐢ㄦ硶: .\scripts\generate-changelog.ps1 [-Version x.y.z] [-Date YYYY-MM-DD] [-NoInsert]
#
# 鍒嗙被瑙勫垯:
#   feat  -> 鏂板 (Added)
#   fix   -> 淇 (Fixed)
#   perf  -> 鎬ц兘 (Performance)
#   refactor -> 鏀硅繘 (Changed)
#   BREAKING CHANGE / ! -> 鐮村潖鎬у彉鏇?(Breaking Changes)

param(
    [string]$Version,
    [string]$Date = (Get-Date -Format "yyyy-MM-dd"),
    [switch]$NoInsert
)

$ErrorActionPreference = "Stop"

# 鑾峰彇涓婃 tag
$lastTag = git describe --tags --abbrev=0 2>$null
if (-not $lastTag) {
    Write-Host "鏈壘鍒板巻鍙?tag锛屽皢鎻愬彇鍏ㄩ儴 commit"
    $range = ""
} else {
    Write-Host "涓婃 tag: $lastTag"
    $range = "$lastTag..HEAD"
}

# 鑾峰彇 commit 鍒楄〃
$commits = git log $range --pretty=format:"%s" 2>$null
if (-not $commits) {
    Write-Host "娌℃湁鏂扮殑 commit"
    exit 0
}

# 鍒嗙被瀹瑰櫒
$added = @()
$fixed = @()
$perf = @()
$changed = @()
$breaking = @()

foreach ($line in $commits) {
    # 璺宠繃 release/chore 鎻愪氦
    if ($line -match '^chore\(release\)') { continue }

    # 妫€娴嬬牬鍧忔€у彉鏇?
    if ($line -match 'BREAKING CHANGE|!') {
        $breaking += $line
        continue
    }

    if ($line -match '^feat') { $added += $line }
    elseif ($line -match '^fix') { $fixed += $line }
    elseif ($line -match '^perf') { $perf += $line }
    elseif ($line -match '^refactor') { $changed += $line }
    # docs/style/test/build/ci/chore 蹇界暐
}

# 鐢熸垚 Markdown
$output = @()
$output += "## [$Version] - $Date"
$output += ""

if ($breaking.Count -gt 0) {
    $output += "### 鐮村潖鎬у彉鏇?
    $output += ""
    foreach ($c in $breaking) { $output += "- $c" }
    $output += ""
}
if ($added.Count -gt 0) {
    $output += "### 鏂板"
    $output += ""
    foreach ($c in $added) { $output += "- $c" }
    $output += ""
}
if ($fixed.Count -gt 0) {
    $output += "### 淇"
    $output += ""
    foreach ($c in $fixed) { $output += "- $c" }
    $output += ""
}
if ($perf.Count -gt 0) {
    $output += "### 鎬ц兘"
    $output += ""
    foreach ($c in $perf) { $output += "- $c" }
    $output += ""
}
if ($changed.Count -gt 0) {
    $output += "### 鏀硅繘"
    $output += ""
    foreach ($c in $changed) { $output += "- $c" }
    $output += ""
}

$result = $output -join "`n"
Write-Host ""
Write-Host "===== 鐢熸垚鐨?CHANGELOG 鏉＄洰 ====="
Write-Host $result
Write-Host "================================"

# 鍐欏叆 CHANGELOG.md
$changelogPath = Join-Path (Get-Location) "CHANGELOG.md"

if ($NoInsert) {
    Write-Host "鉁?--NoInsert 妯″紡锛氬凡杈撳嚭棰勮锛屾湭鍐欏叆鏂囦欢"
    exit 0
}

if (Test-Path $changelogPath) {
    $content = Get-Content $changelogPath -Raw -Encoding UTF8
    # 鍦?[Unreleased] 涔嬪悗鎻掑叆鏂版潯鐩?
    if ($content -match '## \[Unreleased\]') {
        $content = $content -replace '## \[Unreleased\]', "## [Unreleased]`n`n$result"
    } else {
        # 鏃?[Unreleased]锛氬湪鏂囦欢寮€澶达紙鏍囬琛屽悗锛夋彃鍏?
        if ($content -match '^# .+\r?\n') {
            $content = $content -replace '^# .+\r?\n', "&`n`n$result`n"
        } else {
            $content = "$result`n`n$content"
        }
    }
    Set-Content $changelogPath $content -Encoding UTF8
    Write-Host "鉁?CHANGELOG.md 宸叉洿鏂?
} else {
    # 鏂囦欢涓嶅瓨鍦紝鍒涘缓甯﹁鑼冨ご閮ㄧ殑 CHANGELOG
    $header = "# 鏇存柊鏃ュ織`n`n鏈枃浠惰褰曢」鐩殑鎵€鏈夐噸瑕佸彉鏇淬€傛牸寮忓熀浜?[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)锛岀増鏈彿閬靛惊 [璇箟鍖栫増鏈琞(https://semver.org/lang/zh-CN/)銆俙n`n## [Unreleased]`n"
    $content = "$header`n$result`n"
    Set-Content $changelogPath $content -Encoding UTF8
    Write-Host "鉁?CHANGELOG.md 宸插垱寤哄苟鍐欏叆"
}
