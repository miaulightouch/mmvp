//! mmvp-dumper: build the voice site from the JP client.
//!
//!   mmvp-dumper <jp_root> [--out DIR] [--repo OWNER/REPO]
//!
//! Writes DIR/<date> (default work/pages) and prints its path. Deploying it is
//! scripts/publish-voices.mjs (`npm run publish:voices`).
mod dump;

use anyhow::{bail, Result};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let Some(jp_root) = args.first().filter(|a| !a.starts_with("--")) else {
        bail!("usage: mmvp-dumper <jp_root> [--out DIR] [--repo OWNER/REPO]");
    };
    let out = opt("--out").unwrap_or_else(|| "work/pages".into());
    let site = dump::dump(Path::new(jp_root), Path::new(&out), opt("--repo").as_deref())?;
    println!("{}", site.display());
    Ok(())
}
