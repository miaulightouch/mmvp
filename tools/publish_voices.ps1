# One shot: dump JP voices and deploy them to Cloudflare Pages.
#   .\tools\publish_voices.ps1             dump + deploy (skips the deploy if nothing changed)
#   .\tools\publish_voices.ps1 -Force      deploy even if the voices are identical to the live site
#   .\tools\publish_voices.ps1 -DryRun     dump, compare, print the deploy command only
# Settings: deploy.local.json in the repo root (gitignored), e.g.
#   { "project": "mmvoice-jp", "jp_root": "D:\\Nexon\\MabinogiMobile_JP" }
# The live site URL is https://<project>.pages.dev unless "url" is set.
param([switch]$Force, [switch]$DryRun, [int]$Jobs = 0)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Set-Location $repo
$cfgPath = Join-Path $repo 'deploy.local.json'
if (-not (Test-Path $cfgPath)) { throw "missing $cfgPath (gitignored), e.g. {`"project`": `"mmvoice-jp`", `"jp_root`": `"D:\\Nexon\\MabinogiMobile_JP`"}" }
$cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json
$jp = if ($cfg.jp_root) { $cfg.jp_root } else { 'D:\Nexon\MabinogiMobile_JP' }
$url = if ($cfg.url) { $cfg.url.TrimEnd('/') } else { "https://$($cfg.project).pages.dev" }

# the JP launcher deletes index files while it updates / repairs: do not dump a half-written install
if (-not (Test-Path (Join-Path $jp 'AssetPackages\index'))) { throw "$jp\AssetPackages\index missing: is the JP launcher updating?" }

$date = Get-Date -Format 'yyyy-MM-dd'
$dumpArgs = @('tools\dump_site.py', $jp, '--date', $date)
if ($Jobs -gt 0) { $dumpArgs += @('--jobs', $Jobs) }
& .\.venv\Scripts\python.exe @dumpArgs
if ($LASTEXITCODE) { throw "dump failed" }
$site = Join-Path $repo "work\pages\$date"
$new = Get-Content (Join-Path $site 'latest.json') -Raw | ConvertFrom-Json

try {
    $live = Invoke-RestMethod "$url/latest.json" -TimeoutSec 15
    Write-Host "live: $($live.date) $($live.manifest_sha1.Substring(0, 12))  new: $($new.date) $($new.manifest_sha1.Substring(0, 12))"
    if ($live.manifest_sha1 -eq $new.manifest_sha1 -and -not $Force) {
        Write-Host "JP voices unchanged since $($live.date): nothing to deploy (use -Force to deploy anyway)"
        return
    }
} catch {
    Write-Host "live site not reachable ($url): first deploy?"
}
& (Join-Path $PSScriptRoot 'deploy_pages.ps1') -Site $site -DryRun:$DryRun
