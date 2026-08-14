# Collect all release artifacts
$version = "0.6.0"
$artifactsDir = "D:\github\LlamaUI\release_artifacts"
$bundleDir = "D:\github\LlamaUI\target\x86_64-pc-windows-msvc\release\bundle"
$exeDir = "D:\github\LlamaUI\target\x86_64-pc-windows-msvc\release"
$distDir = "D:\github\LlamaUI\dist"

# Clean and recreate
if (Test-Path $artifactsDir) { Remove-Item $artifactsDir -Recurse -Force }
New-Item -ItemType Directory -Path $artifactsDir | Out-Null

# 1. NSIS installer
$nsis = Join-Path $bundleDir "nsis\LlamaUI_${version}_x64-setup.exe"
if (Test-Path $nsis) {
    Copy-Item $nsis (Join-Path $artifactsDir "LlamaUI_${version}_Windows_x64.exe")
    Write-Output "Copied NSIS: LlamaUI_${version}_Windows_x64.exe"
}

# 2. MSI installer
$msi = Join-Path $bundleDir "msi\LlamaUI_${version}_x64_en-US.msi"
if (Test-Path $msi) {
    Copy-Item $msi (Join-Path $artifactsDir "LlamaUI_${version}_Windows_x64.msi")
    Write-Output "Copied MSI: LlamaUI_${version}_Windows_x64.msi"
}

# 3. Portable zip (exe + dist)
$portableDir = Join-Path $env:TEMP "llamaui_portable"
if (Test-Path $portableDir) { Remove-Item $portableDir -Recurse -Force }
New-Item -ItemType Directory -Path $portableDir | Out-Null

$exe = Join-Path $exeDir "llama-ui.exe"
if (Test-Path $exe) {
    Copy-Item $exe $portableDir
}
if (Test-Path $distDir) {
    Copy-Item $distDir (Join-Path $portableDir "dist") -Recurse
}

$zipPath = Join-Path $artifactsDir "LlamaUI_${version}_Windows_x64.zip"
Compress-Archive -Path (Join-Path $portableDir "*") -DestinationPath $zipPath -Force
Write-Output "Created portable zip: LlamaUI_${version}_Windows_x64.zip"

if (Test-Path $portableDir) { Remove-Item $portableDir -Recurse -Force }

# 4. Generate SHA256 checksums
$sha256File = Join-Path $artifactsDir "SHA256SUMS.txt"
$hashes = @()
Get-ChildItem $artifactsDir -File | Where-Object { $_.Name -ne "SHA256SUMS.txt" } | Sort-Object Name | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    $hashes += "$hash  $($_.Name)"
    Write-Output "$hash  $($_.Name)"
}
$hashes | Set-Content $sha256File -Encoding ASCII
Write-Output ""
Write-Output "=== All artifacts ==="
Get-ChildItem $artifactsDir -File | Select-Object Name, @{N='Size(MB)';E={[math]::Round($_.Length/1MB,2)}} | Format-Table -AutoSize
