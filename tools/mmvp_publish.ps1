# Dump JP voices and deploy them to Cloudflare Pages when they changed.
#   .\tools\mmvp_publish.ps1            dump, compare with the live site, deploy on change
#   .\tools\mmvp_publish.ps1 -Force     deploy even if unchanged
#   .\tools\mmvp_publish.ps1 -DryRun    dump and compare only
# deploy.local.json (repo root, gitignored): {"project": "<Pages project>", "url": "<site>", "jp_root": "D:\\Nexon\\MabinogiMobile_JP"}
# Cloudflare login: `npx wrangler login` once, or CLOUDFLARE_API_TOKEN in this shell.
param([switch]$Force, [switch]$DryRun)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$cfg = Get-Content deploy.local.json -Raw | ConvertFrom-Json
$jp = if ($cfg.jp_root) { $cfg.jp_root } else { 'D:\Nexon\MabinogiMobile_JP' }
$url = if ($cfg.url) { $cfg.url.TrimEnd('/') } else { "$($cfg.project).pages.dev" }
if ($url -notmatch '^https?://') { $url = "https://$url" }
$repo = if ((git remote get-url origin) -match 'github\.com[:/](.+?)(\.git)?$') { $Matches[1] }

# the JP launcher deletes index files while it updates: never dump a half-written install
if (-not (Test-Path "$jp\AssetPackages\index")) { throw "$jp\AssetPackages\index missing: is the JP launcher updating?" }

$date = Get-Date -Format 'yyyy-MM-dd'
& .\.venv\Scripts\python.exe tools\mmvp_dump.py $jp --date $date --repo $repo
if ($LASTEXITCODE) { throw 'dump failed' }
$site = "work\pages\$date"
$new = Get-Content "$site\latest.json" -Raw | ConvertFrom-Json
$live = try { Invoke-RestMethod "$url/latest.json" -TimeoutSec 15 } catch { $null }
Write-Host "live: $($live.date) $($live.manifest_sha1)`nnew:  $($new.date) $($new.manifest_sha1)"
if ($live.manifest_sha1 -eq $new.manifest_sha1 -and -not $Force) { Write-Host 'voices unchanged: nothing to deploy'; return }
if ($DryRun) { Write-Host "would deploy $site to Pages project $($cfg.project)"; return }
npx --yes wrangler pages deploy $site --project-name $cfg.project --branch main --commit-dirty=true
