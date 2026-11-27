//! JP voices from the voice site (see mmvp_shared::site): the latest manifest, plus clips downloaded
//! on demand into a local cache and verified by SHA1.
use anyhow::{bail, Context, Result};
use crate::net;
use mmvp_shared::sha1_hex;
use mmvp_shared::site::{Latest, Manifest, VERSION};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The site's base URL, or a local dump directory (testing).
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
        if l.version != VERSION {
            bail!("語音來源格式 {} 不支援，請更新程式", l.version);
        }
        Ok(l)
    }
}

pub struct Voices {
    source: Source,
    /// `latest.manifest_sha1` is the voice version id
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
        Ok(Voices { source, latest, manifest, cache: cache.join("clips") })
    }

    pub fn lookup(&self, bundle: &str, path: &str) -> Option<&str> {
        self.manifest.lookup(bundle, path)
    }

    pub fn fields(&self, sha1: &str) -> Option<&Map<String, Value>> {
        self.manifest.clips.get(sha1).map(|c| &c.fields)
    }

    fn path(&self, sha1: &str) -> PathBuf {
        self.cache.join(sha1)
    }

    /// Download the clips in `need` that are not cached yet (parallel, SHA1-verified).
    pub fn fetch(&self, need: &HashSet<String>, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<()> {
        fs::create_dir_all(&self.cache)?;
        let missing: Vec<&String> = need.iter().filter(|h| !self.path(h).exists()).collect();
        let total: u64 = missing.iter().filter_map(|h| self.manifest.clips.get(*h)).map(|c| c.size).sum();
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
