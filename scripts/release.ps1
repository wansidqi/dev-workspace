# Builds the optimised app and archives it under releases/<yyyy-MM-dd>/ so a new
# release never overwrites an older one. A second release on the same day gets a
# time suffix (releases/2026-11-20_1530/) instead of replacing the first.
#
# Usage (from the repository root):  powershell -File scripts\release.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release -p app
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }

$exe = Join-Path $root 'target\release\devspace.exe'
if (-not (Test-Path $exe)) { throw "missing $exe" }

$name = Get-Date -Format 'yyyy-MM-dd'
$dir = Join-Path $root "releases\$name"
if (Test-Path $dir) {
    $name = Get-Date -Format 'yyyy-MM-dd_HHmmss'
    $dir = Join-Path $root "releases\$name"
}
New-Item -ItemType Directory -Path $dir | Out-Null
Copy-Item $exe $dir

$commit = (git rev-parse --short HEAD).Trim()
$dirty = if (git status --porcelain) { ' (uncommitted changes)' } else { '' }
"devspace build $name`ncommit $commit$dirty" | Set-Content (Join-Path $dir 'BUILD.txt') -Encoding utf8

Write-Host "Release saved to $dir"
