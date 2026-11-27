//! mmvp-dumper: build the voice site from the JP client and publish it to Cloudflare Pages.
//!
//!   mmvp-dumper dump <jp_root> [--out DIR] [--repo OWNER/REPO]   write the site to DIR/<date>
//!   mmvp-dumper publish [--force] [--dry-run]                    dump, compare with the live site,
//!                                                                 deploy with wrangler on change
//! `publish` reads deploy.local.json (gitignored): {"project", "url", "jp_root"}. Cloudflare login:
//! `npx wrangler login` once, or CLOUDFLARE_API_TOKEN.
mod dump;

use anyhow::{bail, Context, Result};
use mmvp_shared::{net, site::Latest};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
struct Deploy {
    project: String,
    url: Option<String>,
    jp_root: Option<PathBuf>,
}

/// GitHub OWNER/REPO of the `origin` remote, linked from the landing page.
fn origin_repo() -> Option<String> {
    let out = Command::new("git").args(["remote", "get-url", "origin"]).output().ok()?;
    let url = String::from_utf8(out.stdout).ok()?;
    let rest = url.trim().split_once("github.com")?.1.trim_start_matches([':', '/']);
    Some(rest.trim_end_matches(".git").to_string())
}

fn publish(force: bool, dry_run: bool) -> Result<()> {
    let cfg: Deploy = serde_json::from_slice(&std::fs::read("deploy.local.json").context("deploy.local.json (gitignored) is missing")?)?;
    let jp = cfg.jp_root.unwrap_or_else(|| PathBuf::from(r"D:\Nexon\MabinogiMobile_JP"));
    let mut url = cfg.url.unwrap_or_else(|| format!("{}.pages.dev", cfg.project));
    if !url.starts_with("http") {
        url = format!("https://{url}");
    }
    let site = dump::dump(&jp, Path::new("work/pages"), origin_repo().as_deref())?;
    let new: Latest = serde_json::from_slice(&std::fs::read(site.join("latest.json"))?)?;
    let live: Option<Latest> = net::get(&format!("{url}/latest.json"), &|_, _| {}).ok().and_then(|b| serde_json::from_slice(&b).ok());
    println!("live: {}\nnew:  {} {}", live.as_ref().map_or("-".into(), |l| format!("{} {}", l.date, l.manifest_sha1)), new.date, new.manifest_sha1);
    if live.is_some_and(|l| l.manifest_sha1 == new.manifest_sha1) && !force {
        println!("voices unchanged: nothing to deploy");
        return Ok(());
    }
    if dry_run {
        println!("would deploy {} to Pages project {}", site.display(), cfg.project);
        return Ok(());
    }
    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let st = Command::new(npx)
        .args(["--yes", "wrangler", "pages", "deploy"])
        .arg(&site)
        .args(["--project-name", &cfg.project, "--branch", "main", "--commit-dirty=true"])
        .status()
        .context("npx (Node.js) not found")?;
    if !st.success() {
        bail!("wrangler failed");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let flag = |k: &str| args.iter().any(|a| a == k);
    match args.first().map(String::as_str) {
        Some("dump") if args.len() >= 2 => {
            let out = opt("--out").unwrap_or_else(|| "work/pages".into());
            let repo = opt("--repo").or_else(origin_repo);
            dump::dump(Path::new(&args[1]), Path::new(&out), repo.as_deref())?;
        }
        Some("publish") => publish(flag("--force"), flag("--dry-run"))?,
        _ => bail!("usage:\n  mmvp-dumper dump <jp_root> [--out DIR] [--repo OWNER/REPO]\n  mmvp-dumper publish [--force] [--dry-run]"),
    }
    Ok(())
}
