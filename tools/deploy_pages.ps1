# Deploy a dumped voice site (tools/dump_site.py output) to Cloudflare Pages with wrangler.
#   .\tools\deploy_pages.ps1 work\pages\2026-11-27
# Settings come from deploy.local.json in the repo root (gitignored), e.g.
#   { "project": "mmvoice-jp" }
# Authentication: `npx wrangler login` once, or set CLOUDFLARE_API_TOKEN (+ CLOUDFLARE_ACCOUNT_ID) in this
# shell. Nothing about Cloudflare is ever committed.
param([Parameter(Mandatory)][string]$Site, [switch]$DryRun)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$cfgPath = Join-Path $repo 'deploy.local.json'
if (-not (Test-Path $cfgPath)) { throw "missing $cfgPath (gitignored), e.g. {`"project`": `"mmvoice-jp`"}" }
$cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json
foreach ($f in 'latest.json', 'manifest.json', '_headers') {
    if (-not (Test-Path (Join-Path $Site $f))) { throw "$Site is not a dump_site.py output (no $f)" }
}
$n = (Get-ChildItem $Site -Recurse -File).Count
if ($n -gt 20000) { throw "$n files: Cloudflare Pages allows at most 20000 per deployment" }
$latest = Get-Content (Join-Path $Site 'latest.json') -Raw | ConvertFrom-Json
Write-Host "deploy $($latest.date): $($latest.clips) clips, $n files -> Pages project '$($cfg.project)'"
$wargs = @('wrangler', 'pages', 'deploy', $Site, '--project-name', $cfg.project, '--branch', 'main', '--commit-dirty=true')
if ($DryRun) { Write-Host "npx $($wargs -join ' ')"; return }
npx --yes @wargs
