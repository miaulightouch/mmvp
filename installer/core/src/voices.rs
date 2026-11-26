//! JP voice source: a static site made by tools/dump_site.py (Cloudflare Pages in production,
//! or a local directory for testing):
//!   latest.json      {"version": 3, "date", "manifest", "manifest_sha1", "clips", "bytes"}
//!   manifest.json    {"version": 3, "clips": {sha1: {"size", "fields"}}, "bundles", "default"}
//!                    content only, so its SHA1 (latest.manifest_sha1) identifies the voice version
//!   clips/<sha1>     raw FSB5 bytes, content-addressed
//! Clips are downloaded on demand (only those a patch needs) into a local cache and verified by SHA1.
use crate::audio::sha1_hex;
use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Latest {
    pub version: u32,
    pub date: String,
    pub manifest: String,
    pub manifest_sha1: String,
    #[serde(default)]
    pub clips: usize,
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Deserialize)]
pub struct ClipMeta {
    pub size: u64,
    pub fields: Map<String, Value>,
}

#[derive(Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub clips: HashMap<String, ClipMeta>,
    pub bundles: HashMap<String, HashMap<String, String>>,
    pub default: HashMap<String, String>,
}

impl Manifest {
    /// JP variant for this exact bundle if JP has it, else the most common variant of the path.
    pub fn lookup(&self, bundle: &str, path: &str) -> Option<&str> {
        self.bundles.get(bundle).and_then(|m| m.get(path)).or_else(|| self.default.get(path)).map(|s| s.as_str())
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    Http(String),
    Dir(PathBuf),
}

impl Source {
    /// "https://..." / "http://..." or a local directory.
    pub fn parse(s: &str) -> Source {
        if s.starts_with("https://") || s.starts_with("http://") {
            Source::Http(s.trim_end_matches('/').to_string())
        } else {
            Source::Dir(PathBuf::from(s))
        }
    }
    pub fn describe(&self) -> String {
        match self {
            Source::Http(u) => u.clone(),
            Source::Dir(p) => p.display().to_string(),
        }
    }
    fn get(&self, rel: &str) -> Result<Vec<u8>> {
        match self {
            Source::Http(base) => {
                let url = format!("{base}/{rel}");
                let resp = ureq::get(&url).timeout(Duration::from_secs(60)).call().with_context(|| format!("GET {url}"))?;
                let mut v = Vec::new();
                resp.into_reader().take(64 << 20).read_to_end(&mut v)?;
                Ok(v)
            }
            Source::Dir(d) => fs::read(d.join(rel)).with_context(|| format!("read {}", d.join(rel).display())),
        }
    }
}

pub fn fetch_latest(src: &Source) -> Result<Latest> {
    let l: Latest = serde_json::from_slice(&src.get("latest.json")?).context("bad latest.json")?;
    if l.version != 3 {
        bail!("語音來源格式版本 {} 不支援（需要 3），請更新程式", l.version);
    }
    Ok(l)
}

pub struct Voices {
    pub source: Source,
    pub latest: Latest,
    pub manifest: Manifest,
    /// manifest SHA1: identifies the voice version
    pub id: String,
    cache: PathBuf,
}

impl Voices {
    /// Load the latest voice version from `src`, falling back to the cached copy when offline.
    pub fn open(src: Source, cache: &Path) -> Result<Voices> {
        fs::create_dir_all(cache.join("clips"))?;
        let online = fetch_latest(&src).and_then(|l| {
            let raw = src.get(&l.manifest)?;
            if sha1_hex(&raw) != l.manifest_sha1 {
                bail!("manifest.json SHA1 不符（部署中？請稍後再試）");
            }
            Ok((l, raw))
        });
        let (latest, raw) = match online {
            Ok((l, raw)) => {
                let _ = fs::write(cache.join("manifest.json"), &raw);
                let _ = fs::write(cache.join("latest.json"), serde_json::to_vec(&l)?);
                (l, raw)
            }
            Err(e) => {
                let l: Latest = fs::read(cache.join("latest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
                    .ok_or_else(|| anyhow!("無法取得語音資料：{e:#}"))?;
                let raw = fs::read(cache.join("manifest.json")).map_err(|_| anyhow!("無法取得語音資料：{e:#}"))?;
                if sha1_hex(&raw) != l.manifest_sha1 {
                    bail!("無法取得語音資料：{e:#}");
                }
                (l, raw)
            }
        };
        let manifest: Manifest = serde_json::from_slice(&raw).context("bad manifest.json")?;
        if manifest.version != 3 {
            bail!("manifest 格式版本 {} 不支援", manifest.version);
        }
        Ok(Voices { source: src, id: latest.manifest_sha1.clone(), latest, manifest, cache: cache.to_path_buf() })
    }

    fn clip_path(&self, sha1: &str) -> PathBuf {
        self.cache.join("clips").join(sha1)
    }

    /// Download every clip in `need` that is not cached yet (parallel, SHA1-verified).
    /// `progress(done_bytes, total_bytes)`.
    pub fn ensure(&self, need: &HashSet<String>, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<usize> {
        fs::create_dir_all(self.cache.join("clips"))?; // the cache may have been cleared while the app runs
        let missing: Vec<&String> = need.iter().filter(|h| !self.clip_path(h).exists()).collect();
        let total: u64 = missing.iter().map(|h| self.manifest.clips.get(*h).map_or(0, |c| c.size)).sum();
        let done = AtomicUsize::new(0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
        pool.install(|| {
            missing.par_iter().try_for_each(|h| -> Result<()> {
                let data = self.source.get(&format!("clips/{h}"))?;
                if sha1_hex(&data) != **h {
                    bail!("clip {h} SHA1 不符");
                }
                let p = self.clip_path(h);
                let tmp = p.with_extension("part");
                fs::write(&tmp, &data)?;
                fs::rename(&tmp, &p)?;
                let d = done.fetch_add(data.len(), Ordering::Relaxed) + data.len();
                progress(d as u64, total);
                Ok(())
            })
        })?;
        Ok(missing.len())
    }

    pub fn clip(&self, sha1: &str) -> Result<Vec<u8>> {
        let v = fs::read(self.clip_path(sha1)).with_context(|| format!("clip {sha1} 不在快取"))?;
        if sha1_hex(&v) != sha1 {
            let _ = fs::remove_file(self.clip_path(sha1));
            bail!("快取的 clip {sha1} 損壞，請重試");
        }
        Ok(v)
    }

    /// Remove cached clips the current manifest no longer uses.
    pub fn prune_cache(&self) {
        if let Ok(rd) = fs::read_dir(self.cache.join("clips")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if !self.manifest.clips.contains_key(&n) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
}
