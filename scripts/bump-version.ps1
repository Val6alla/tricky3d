# Sets the app version everywhere it is defined: package.json, src-tauri/Cargo.toml and
# src-tauri/tauri.conf.json, plus the two lock files that repeat it. The About dialog reads
# package.json. scripts/bump-version.sh does the same on macOS and Linux.
#
#   powershell -ExecutionPolicy Bypass -File scripts\bump-version.ps1 0.3.0

param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'

# Plain numbers only: the Windows .msi installer accepts no other version.
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Give the version as three numbers, for example 0.3.0 (got '$Version')." }
$root = Split-Path $PSScriptRoot -Parent

# Each pattern's first group ends right before the app's own version, never a dependency's. Every
# file is checked before any is written, so a file that doesn't match leaves all of them as they were.
$edits = @(
  @('package.json', '(?m)^(  "version": ")[^"]*'),
  @('package-lock.json', '("name": "tricky3d",\s*"version": ")[^"]*'),
  @('src-tauri/Cargo.toml', '(?m)^(version = ")[^"]*'),
  @('src-tauri/Cargo.lock', '(name = "tricky3d"\r?\nversion = ")[^"]*'),
  @('src-tauri/tauri.conf.json', '(?m)^(  "version": ")[^"]*')
)
$texts = foreach ($edit in $edits) {
  $text = [IO.File]::ReadAllText((Join-Path $root $edit[0]))
  if ($text -notmatch $edit[1]) { throw "$($edit[0]): no version line found." }
  [regex]::Replace($text, $edit[1], '${1}' + $Version)
}
for ($i = 0; $i -lt $edits.Count; $i++) {
  # Writes UTF-8 without a byte order mark, like the files already are.
  [IO.File]::WriteAllText((Join-Path $root $edits[$i][0]), $texts[$i])
}
Write-Output "Version set to $Version. Tag the release as v$Version."
