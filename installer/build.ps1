# Build the installer and lay out a distributable folder in installer\dist.
#   .\build.ps1                         exe + WebView2Loader.dll
#   .\build.ps1 -Pack ..\work\jp_voice.mmvpack -UpdateUrl https://.../update.json
param([string]$Pack, [string]$UpdateUrl)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
cargo build --release -p mmvoice-app -p mmv-cli
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$dist = Join-Path $PSScriptRoot 'dist'
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory $dist | Out-Null
Copy-Item target\release\mmvoice.exe, target\release\WebView2Loader.dll $dist
if ($Pack) { Copy-Item $Pack (Join-Path $dist 'jp_voice.mmvpack') }
if ($UpdateUrl) { @{ update_url = $UpdateUrl } | ConvertTo-Json | Set-Content (Join-Path $dist 'mmvoice.json') -Encoding utf8 }
Get-ChildItem $dist | Select-Object Name, Length
