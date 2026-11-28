//! JP voices from the voice site (see mmvp_shared::site): the latest manifest, plus clips downloaded
//! on demand as per-bundle packs into a local per-clip cache, verified by SHA1.
use anyhow::{bail, Context, Result};
use crate::net;
use mmvp_shared::sha1_hex;
use mmvp_shared::site::{Latest, Manifest, Pack, Packs, LATEST, MANIFEST, PACKS, VERSION};
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
        // the version first: other fields may differ between formats
        let raw: serde_json::Value = serde_json::from_slice(&self.get(LATEST)?).context(LATEST)?;
        if raw.get("version").and_then(|v| v.as_u64()) != Some(VERSION.into()) {
            bail!(tr!("Voice source format {} is not supported; update the program", "語音來源格式不符（{}），請更新程式", raw.get("version").unwrap_or(&serde_json::Value::Null)));
        }
        serde_json::from_value(raw).context(LATEST)
    }
}

pub struct Voices {
    source: Source,
    /// `latest.manifest_sha1` is the voice version id
    pub latest: Latest,
    manifest: Manifest,
    packs: Packs,
    cache: PathBuf,
}

impl Voices {
    pub fn open(source: Source, cache: &Path) -> Result<Voices> {
        let latest = source.latest()?;
        let raw = source.get(MANIFEST)?;
        if sha1_hex(&raw) != latest.manifest_sha1 {
            bail!(tr!("manifest.json does not match latest.json (voices being updated? try again later)", "manifest.json 與 latest.json 不符（語音正在更新？請稍後再試）"));
        }
        let manifest: Manifest = serde_json::from_slice(&raw).context(MANIFEST)?;
        let packs: Packs = serde_json::from_slice(&source.get(PACKS)?).context(PACKS)?;
        if packs.manifest_sha1 != latest.manifest_sha1 {
            bail!(tr!("packs.json does not match latest.json (voices being updated? try again later)", "packs.json 與 latest.json 不符（語音正在更新？請稍後再試）"));
        }
        Ok(Voices { source, latest, manifest, packs, cache: cache.join("clips") })
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

    /// Download the packs holding the clips in `need` that are not cached yet (parallel), verify each
    /// pack and clip by SHA1 and cache the pack's clips.
    pub fn fetch(&self, need: &HashSet<String>, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<()> {
        fs::create_dir_all(&self.cache)?;
        let missing: HashSet<&str> = need.iter().map(String::as_str).filter(|h| !self.path(h).exists()).collect();
        let packs: Vec<(&String, &Pack)> =
            self.packs.packs.iter().filter(|(_, p)| p.clips.iter().any(|(h, _)| missing.contains(h.as_str()))).collect();
        let packed: HashSet<&str> = packs.iter().flat_map(|(_, p)| p.clips.iter().map(|(h, _)| h.as_str())).collect();
        if let Some(h) = missing.iter().find(|h| !packed.contains(*h)) {
            bail!(tr!("voice {h} is in no pack", "語音 {h} 不在任何語音包裡"));
        }
        let size = |h: &str| self.manifest.clips.get(h).map_or(0, |c| c.size as usize);
        let total: u64 = packs.iter().map(|(_, p)| p.size).sum();
        let done = AtomicU64::new(0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(net::PARALLEL).build()?;
        pool.install(|| {
            packs.par_iter().try_for_each(|(ph, p)| -> Result<()> {
                let data = self.source.get(&format!("packs/{ph}"))?;
                if sha1_hex(&data) != **ph {
                    bail!(tr!("voice pack {ph}: incomplete download", "語音包 {ph} 下載不完整"));
                }
                for (h, off) in &p.clips {
                    let clip = usize::try_from(*off).ok().and_then(|o| data.get(o..o + size(h)));
                    let Some(clip) = clip.filter(|c| sha1_hex(c) == *h) else {
                        bail!(tr!("voice pack {ph} does not match the manifest", "語音包 {ph} 與 manifest 不符"));
                    };
                    let tmp = self.path(h).with_extension("part");
                    fs::write(&tmp, clip)?;
                    fs::rename(&tmp, self.path(h))?;
                }
                progress(done.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64, total);
                Ok(())
            })
        })
    }

    pub fn clip(&self, sha1: &str) -> Result<Vec<u8>> {
        let data = fs::read(self.path(sha1)).with_context(|| tr!("voice {sha1} is not in the cache", "語音 {sha1} 不在快取"))?;
        if sha1_hex(&data) != sha1 {
            let _ = fs::remove_file(self.path(sha1));
            bail!(tr!("cached voice {sha1} is damaged; apply again", "快取的語音 {sha1} 損壞，請重新套用"));
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
