//! mmvoice-rs: command-line front end of mmv-core.
//!
//!   mmvoice-rs patch   <tw_root> <source> [--dry-run] [--rescan] [--blob-no N] [--data DIR]
//!   mmvoice-rs restore <tw_root>
//!   mmvoice-rs status  <tw_root>
//! <source> = the voice site (https://<project>.pages.dev) or a local tools/dump_site.py output directory.
use anyhow::{bail, Result};
use mmv_core::patcher::{self, Event, Options};
use mmv_core::voices::{Source, Voices};
use std::path::PathBuf;
use std::sync::Mutex;

fn default_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("mmvoice")
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
            if a == "--blob-no" || a == "--data" || a == "--limit" { skip = true; continue; }
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
            let data = opt("--data").map(PathBuf::from).unwrap_or_else(default_data);
            let voices = Voices::open(Source::parse(pos[2]), &data.join("voices"))?;
            println!("voices {} ({} clips, {})", voices.latest.date, voices.manifest.clips.len(), &voices.id[..12]);
            let blob_no = opt("--blob-no").map(|s| s.parse()).transpose()?.unwrap_or(patcher::DEFAULT_BLOB_NO);
            let cache = data.join("scan_cache.json");
            patcher::patch(&Options { root: pos[1].as_ref(), voices: &voices, cache: &cache, rescan: flag("--rescan"), dry_run: flag("--dry-run"), blob_no }, &report)?;
        }
        Some("fetch") if pos.len() == 2 => {
            // download every clip of the latest version into --data (benchmark / cache warm-up)
            let data = opt("--data").map(PathBuf::from).unwrap_or_else(default_data);
            let voices = Voices::open(Source::parse(pos[1]), &data.join("voices"))?;
            let mut keys: Vec<&String> = voices.manifest.clips.keys().collect();
            keys.sort();
            let limit = opt("--limit").map(|s| s.parse()).transpose()?.unwrap_or(keys.len());
            let all: std::collections::HashSet<String> = keys.into_iter().take(limit).cloned().collect();
            let t0 = std::time::Instant::now();
            let n = voices.ensure(&all, &|_, _| {})?;
            let secs = t0.elapsed().as_secs_f64();
            let mb: u64 = all.iter().filter_map(|h| voices.manifest.clips.get(h)).map(|c| c.size).sum();
            println!("fetched {n} clips ({:.1} MB) in {secs:.1}s", mb as f64 / 1048576.0);
        }
        Some("restore") if pos.len() == 2 => patcher::restore(pos[1].as_ref(), &report)?,
        Some("status") if pos.len() == 2 => {
            for s in patcher::status(pos[1].as_ref())? {
                println!("{}: {:?} ({} entries redirected)", s.pkg, s.state, s.entries_redirected);
            }
        }
        _ => bail!("usage:\n  mmvoice-rs patch <tw_root> <source> [--dry-run] [--rescan] [--blob-no N] [--data DIR]\n  mmvoice-rs restore <tw_root>\n  mmvoice-rs status <tw_root>\n<source>: https://<project>.pages.dev or a dump_site.py output directory"),
    }
    Ok(())
}
