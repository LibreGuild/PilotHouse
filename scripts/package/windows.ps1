# Builds the Windows installer and portable zip into dist\.
#
#   pwsh scripts/package/windows.ps1
#
# Needs NSIS (makensis) on PATH: `choco install nsis` or `winget install NSIS.NSIS`.
# The icon is built into pilothouse.exe by crates/app/build.rs.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force dist | Out-Null

cargo build --release -p pilothouse
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Portable: unzip and run.
$portable = Join-Path ([System.IO.Path]::GetTempPath()) "PilotHouse-portable"
Remove-Item -Recurse -Force $portable -ErrorAction SilentlyContinue
New-Item -ItemType Directory $portable | Out-Null
Copy-Item target\release\pilothouse.exe, LICENSE, NOTICE $portable
Compress-Archive -Force -Path "$portable\*" -DestinationPath dist\PilotHouse-Windows-Portable-x64.zip

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) { $makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe" }
# Windows' file version is numbers only: 1.0.0 for 1.0.0-beta.
$numeric = ($version -split '-')[0]
& $makensis /V2 "/DVERSION=$version" "/DNUMERIC=$numeric" scripts\package\windows.nsi
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
Get-ChildItem dist | Select-Object Name, Length
