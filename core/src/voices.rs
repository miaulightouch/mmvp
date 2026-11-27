//! JP voice source: the static site made by tools/mmvp_dump.py (Cloudflare Pages, or a local directory
//! for testing):
//!   latest.json      {"version": 3, "date", "manifest", "manifest_sha1", "clips", "bytes"}
//!   manifest.json    {"version": 3, "clips": {sha1: {"size", "fields"}}, "bundles", "default"}
//!                    content only, so its SHA1 identifies the voice version
//!   clips/<sha1>     raw FSB5 bytes, content-addressed
//! Clips are downloaded on demand into a local cache and verified by SHA1.
use crate::{net, sha1_hex};
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Deserialize, Clone, Debug)]
pub struct Latest {
    pub version: u32,
    pub date: String,
    pub manifest: String,
    pub manifest_sha1: String,
    pub clips: usize,
    pub bytes: u64,
}

#[derive(Deserialize)]
pub struct Clip {
    pub size: u64,
    pub fields: Map<String, Value>,
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    clips: HashMap<String, Clip>,
    bundles: HashMap<String, HashMap<String, String>>,
    default: HashMap<String, String>,
}

/// "https://..." / "http://..." or a local directory.
#[derive(Clone, Debug)]
pub struct Source(String);

impl Source {
    pub fn new(s: &str) -> Self {
        Source(s.trim_end_matches('/').to_string())
    }
    fn get(&self, rel: &str) -> Result<Vec<u8>> {
        if self.0.starts_with("http://") || self.0.starts_with("https://") {
            net::get(&format!("{}/{rel}", self.0), &|_, _| {})
        } else {
            fs::read(Path::new(&self.0).join(rel)).with_context(|| format!("{}/{rel}", self.0))
        }
    }
    pub fn latest(&self) -> Result<Latest> {
        let l: Latest = serde_json::from_slice(&self.get("latest.json")?).context("latest.json")?;
        if l.version != 3 {
            bail!("語音來源格式 {} 不支援，請更新程式", l.version);
        }
        Ok(l)
    }
}

pub struct Voices {
    source: Source,
    pub latest: Latest,
    manifest: Manifest,
    cache: PathBuf,
}

impl Voices {
    pub fn open(source: Source, cache: &Path) -> Result<Voices> {
        let latest = source.latest()?;
        let raw = source.get(&latest.manifest)?;
        if sha1_hex(&raw) != latest.manifest_sha1 {
            bail!("manifest.json 與 latest.json 不符（語音正在更新？請稍後再試）");
        }
        let manifest: Manifest = serde_json::from_slice(&raw).context("manifest.json")?;
        if manifest.version != 3 {
            bail!("manifest 格式 {} 不支援，請更新程式", manifest.version);
        }
        Ok(Voices { source, latest, manifest, cache: cache.join("clips") })
    }

    /// Voice version id (manifest SHA1).
    pub fn id(&self) -> &str {
        &self.latest.manifest_sha1
    }

    /// JP clip for a container path: the variant of the same bundle if JP has it, else the most common one.
    pub fn lookup(&self, bundle: &str, path: &str) -> Option<&str> {
        let m = &self.manifest;
        m.bundles.get(bundle).and_then(|b| b.get(path)).or_else(|| m.default.get(path)).map(String::as_str)
    }

    pub fn fields(&self, sha1: &str) -> Option<&Map<String, Value>> {
        self.manifest.clips.get(sha1).map(|c| &c.fields)
    }

    pub fn size(&self, sha1: &str) -> u64 {
        self.manifest.clips.get(sha1).map_or(0, |c| c.size)
    }

    fn path(&self, sha1: &str) -> PathBuf {
        self.cache.join(sha1)
    }

    /// Download the clips in `need` that are not cached yet (parallel, SHA1-verified).
    pub fn fetch(&self, need: &HashSet<String>, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<()> {
        fs::create_dir_all(&self.cache)?;
        let missing: Vec<&String> = need.iter().filter(|h| !self.path(h).exists()).collect();
        let total: u64 = missing.iter().map(|h| self.size(h)).sum();
        let done = AtomicU64::new(0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(net::PARALLEL).build()?;
        pool.install(|| {
            missing.par_iter().try_for_each(|h| -> Result<()> {
                let data = self.source.get(&format!("clips/{h}"))?;
                if sha1_hex(&data) != **h {
                    bail!("語音 {h} 下載不完整");
                }
                let tmp = self.path(h).with_extension("part");
                fs::write(&tmp, &data)?;
                fs::rename(&tmp, self.path(h))?;
                progress(done.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64, total);
                Ok(())
            })
        })
    }

    pub fn clip(&self, sha1: &str) -> Result<Vec<u8>> {
        let data = fs::read(self.path(sha1)).with_context(|| format!("語音 {sha1} 不在快取"))?;
        if sha1_hex(&data) != sha1 {
            let _ = fs::remove_file(self.path(sha1));
            bail!("快取的語音 {sha1} 損壞，請重新套用");
        }
        Ok(data)
    }

    /// Drop cached clips the current version does not use.
    pub fn prune_cache(&self) {
        for e in fs::read_dir(&self.cache).into_iter().flatten().flatten() {
            if !self.manifest.clips.contains_key(e.file_name().to_string_lossy().as_ref()) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}
