# Builds the optimised app and archives it under releases/<yyyy-MM-dd>_<name>/ so
# a new release never overwrites an older one. <name> is a short feature name
# (kebab-case), e.g. releases/2026-11-22_github-markdown/. If that folder already
# exists, -2, -3 ... is appended. Without -Name the folder is just the date, and
# a repeat on the same day gets a time suffix instead.
#
# Usage (from the repository root):
#   powershell -File scripts\release.ps1 -Name github-markdown

param([string]$Name = '')

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release -p app
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }

$exe = Join-Path $root 'target\release\devspace.exe'
if (-not (Test-Path $exe)) { throw "missing $exe" }

# Keep the feature name safe as a folder name: lowercase, letters, digits, hyphens.
$feature = ($Name.ToLower() -replace '[^a-z0-9]+', '-').Trim('-')
$date = Get-Date -Format 'yyyy-MM-dd'
$base = if ($feature) { "${date}_$feature" } else { $date }
$name = $base
$dir = Join-Path $root "releases\$name"
if (Test-Path $dir) {
    if ($feature) {
        $n = 2
        while (Test-Path (Join-Path $root "releases\$base-$n")) { $n++ }
        $name = "$base-$n"
    } else {
        $name = Get-Date -Format 'yyyy-MM-dd_HHmmss'
    }
    $dir = Join-Path $root "releases\$name"
}
New-Item -ItemType Directory -Path $dir | Out-Null
Copy-Item $exe $dir

$commit = (git rev-parse --short HEAD).Trim()
$dirty = if (git status --porcelain) { ' (uncommitted changes)' } else { '' }
"devspace build $name`ncommit $commit$dirty" | Set-Content (Join-Path $dir 'BUILD.txt') -Encoding utf8

Write-Host "Release saved to $dir"
