# Local development build (releases are built by .github/workflows/release.yml).
#   .\build.ps1 -Source https://<project>.pages.dev     voice site baked into the exe
#   .\build.ps1 -Source ..\work\pages\2026-11-27        or a local dump_site.py output, for testing
#   .\build.ps1 -Bundle                                 also build the NSIS installer (unsigned: no updater artifacts)
# The app can also be pointed at a source at run time with the MMVOICE_SOURCE environment variable.
param([string]$Source, [switch]$Bundle)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if ($Source) { $env:MMVOICE_SOURCE = $Source }
if ($Bundle) {
    Push-Location app
    if (-not (Test-Path node_modules)) { npm ci }
    Remove-Item ..\target\release\bundle\nsis\*.exe* -ErrorAction SilentlyContinue
    $cfg = Join-Path ([IO.Path]::GetTempPath()) 'mmvoice-nosign.json'
    '{"bundle":{"createUpdaterArtifacts":false}}' | Set-Content $cfg
    npx tauri build --config $cfg
    Pop-Location
} else {
    cargo build --release -p mmvoice-app -p mmv-cli
}
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$dist = Join-Path $PSScriptRoot 'dist'
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory $dist | Out-Null
Copy-Item target\release\mmvoice.exe, target\release\mmvoice-rs.exe $dist
if (Test-Path target\release\WebView2Loader.dll) { Copy-Item target\release\WebView2Loader.dll $dist }  # GNU toolchain only
Get-ChildItem target\release\bundle\nsis\*.exe -ErrorAction SilentlyContinue | Copy-Item -Destination $dist
Get-ChildItem $dist | Select-Object Name, Length
