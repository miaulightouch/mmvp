//! mmvoice-rs: command-line front end of mmv-core.
//!
//!   mmvoice-rs patch   <tw_root> <pack> [--dry-run] [--rescan] [--blob-no N] [--cache FILE]
//!   mmvoice-rs restore <tw_root>
//!   mmvoice-rs status  <tw_root>
use anyhow::{bail, Result};
use mmv_core::pack::Pack;
use mmv_core::patcher::{self, Event, Options};
use std::path::PathBuf;
use std::sync::Mutex;

fn default_cache() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("mmvoice").join("scan_cache.json")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let opt = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let pos: Vec<&String> = {
        let mut v = Vec::new();
        let mut skip = false;
        for a in &args {
            if skip { skip = false; continue; }
            if a == "--blob-no" || a == "--cache" { skip = true; continue; }
            if !a.starts_with("--") { v.push(a); }
        }
        v
    };
    let last = Mutex::new(String::new());
    let report = |e: Event| match e {
        Event::Stage(s) => eprintln!("== {s}"),
        Event::Progress { stage, done, total } => {
            if done == total || done % 50 == 0 {
                let line = format!("[{stage}] {done}/{total}");
                let mut l = last.lock().unwrap();
                if *l != line { eprintln!("{line}"); *l = line; }
            }
        }
        Event::Warn(w) => eprintln!("[warn] {w}"),
        Event::Info(i) => println!("{i}"),
    };
    match pos.first().map(|s| s.as_str()) {
        Some("patch") if pos.len() == 3 => {
            let pack = Pack::open(pos[2].as_ref())?;
            println!("pack {} ({} clips, created {})", &pack.id[..12], pack.manifest.clips.len(), pack.manifest.created);
            let cache = opt("--cache").map(PathBuf::from).unwrap_or_else(default_cache);
            let blob_no = opt("--blob-no").map(|s| s.parse()).transpose()?.unwrap_or(patcher::DEFAULT_BLOB_NO);
            patcher::patch(&Options { root: pos[1].as_ref(), pack: &pack, cache: &cache, rescan: flag("--rescan"), dry_run: flag("--dry-run"), blob_no }, &report)?;
        }
        Some("restore") if pos.len() == 2 => patcher::restore(pos[1].as_ref(), &report)?,
        Some("status") if pos.len() == 2 => {
            for s in patcher::status(pos[1].as_ref())? {
                println!("{}: {:?} ({} entries redirected)", s.pkg, s.state, s.entries_redirected);
            }
        }
        _ => bail!("usage:\n  mmvoice-rs patch <tw_root> <pack> [--dry-run] [--rescan] [--blob-no N] [--cache FILE]\n  mmvoice-rs restore <tw_root>\n  mmvoice-rs status <tw_root>"),
    }
    Ok(())
}
