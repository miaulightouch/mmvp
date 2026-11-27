//! A TW install: status, apply, restore.
//!
//! Apply = scan the referenced audio bundles, download the JP clips that differ, rebuild those bundles
//! into `<pkg>/9876.blob` (or 9877, whichever the live index does not use) and redirect every index
//! location of a rebuilt bundle to its new slot. Original blobs are never touched.
//!
//! State lives in the install itself:
//!  * an index is patched if a location points at a blob >= 5000; its original is `<pkg>/index.bak`
//!    (only ever written from an unpatched index);
//!  * every `NNNN.blob` >= 5000 in a package directory is ours;
//!  * `<pkg>/index.mmvp.json` records the voice version and the original blobs used, to tell when
//!    a re-apply is needed.
use crate::voices::Voices;
use mmvp_shared::index::{self, Key, REDIRECT_BASE};
use mmvp_shared::{audio, blob, blob_no, sha1_hex};
use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

pub const GAME_EXE: &str = "MabinogiMobile.exe";
const BLOB_NO: u16 = 9876;
const MARKER: &str = "index.mmvp.json";
/// [base package, Starter sub-package]; bit 15 of an index location selects the member.
const FAMILIES: [[&str; 2]; 2] = [["AssetPackages", "AssetPackages/Starter"], ["ResourcesPackage", "ResourcesPackage/Starter"]];

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Scan,
    Download,
    Rebuild,
    Commit,
}
/// `(step, done, total)`
pub type Progress<'a> = &'a (dyn Fn(Step, u64, u64) + Sync);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Original,
    Patched,
    /// a game update or launcher repair replaced the patched index
    Reverted,
    /// an index is missing: the launcher is updating
    Missing,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub state: State,
    /// asset entries pointing at our blobs
    pub redirected: usize,
    /// voice version (id, date) the install was patched with, if known
    pub voice: Option<(String, String)>,
    /// original blobs changed since apply (game update that kept the index): re-apply needed
    pub stale: bool,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    voice_id: String,
    voice_date: String,
    index_sha1: String,
    /// "pkg/NNNN.blob" -> (size, mtime)
    sources: BTreeMap<String, (u64, u64)>,
}

struct Pkg {
    name: &'static str,
    member: usize, // 0 = base, 1 = Starter
    family: usize,
    dir: PathBuf,
    live: Vec<u8>,
    orig: Vec<u8>,
    patched: bool,
}

/// A bundle to rebuild: `<pkg>/<blob>.blob` slot, with the JP clip per container path.
struct Item {
    pkg: usize,
    blob: u16,
    slot: u16,
    jp: Vec<(String, String)>,
}

pub struct Install {
    pub root: PathBuf,
}


fn stat(p: &Path) -> Result<(u64, u64)> {
    let m = fs::metadata(p).with_context(|| p.display().to_string())?;
    Ok((m.len(), m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos() as u64))
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("mmvp.tmp");
    fs::write(&tmp, data).with_context(|| tmp.display().to_string())?;
    fs::rename(&tmp, path).with_context(|| path.display().to_string())
}

fn read_slot(path: &Path, slot: u16) -> Result<Vec<u8>> {
    let (s, e) = *blob::read_table(path)?.get(slot as usize).ok_or_else(|| anyhow!("{}: no slot {slot}", path.display()))?;
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(s))?;
    let mut buf = vec![0; (e - s) as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Our blobs (>= 5000) in a package directory.
fn our_blobs(dir: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(dir).into_iter().flatten().flatten();
    entries.filter(|e| blob_no(&e.file_name().to_string_lossy()).is_some_and(|n| n >= REDIRECT_BASE)).map(|e| e.path()).collect()
}

impl Install {
    /// `root` is the folder that holds AssetPackages (…\MabinogiMobile_TW\appdata).
    pub fn open(root: &Path) -> Result<Install> {
        let ap = root.join("AssetPackages");
        if !ap.join("index").exists() && !ap.join("index.bak").exists() {
            bail!("{} 不是瑪奇 Mobile 的安裝目錄（找不到 AssetPackages\\index）", root.display());
        }
        Ok(Install { root: root.to_path_buf() })
    }

    /// True if this install's game is running: Windows refuses write access to a running exe.
    pub fn game_running(&self) -> bool {
        let exe = fs::OpenOptions::new().write(true).open(self.root.join(GAME_EXE));
        matches!(exe, Err(e) if e.raw_os_error() == Some(32)) // ERROR_SHARING_VIOLATION
    }

    fn pkgs(&self) -> Result<Vec<Pkg>> {
        let mut out = Vec::new();
        for (family, members) in FAMILIES.iter().enumerate() {
            for (member, name) in members.iter().enumerate() {
                let dir = self.root.join(name.replace('/', "\\"));
                if !dir.exists() {
                    continue;
                }
                let live = fs::read(dir.join("index")).with_context(|| format!("{name}\\index 不存在（launcher 正在更新？）"))?;
                let patched = index::is_patched(&live).with_context(|| format!("{name}\\index"))?;
                let orig = if patched {
                    let bak = fs::read(dir.join("index.bak")).with_context(|| format!("{name}\\index.bak 不存在，請用 launcher 修復遊戲"))?;
                    if index::is_patched(&bak)? {
                        bail!("{name}\\index.bak 不是原始檔，請用 launcher 修復遊戲");
                    }
                    bak
                } else {
                    live.clone()
                };
                out.push(Pkg { name, member, family, dir, live, orig, patched });
            }
        }
        Ok(out)
    }

    pub fn status(&self) -> Result<Status> {
        let mut st = Status { state: State::Original, redirected: 0, voice: None, stale: false };
        let (mut patched, mut ours, mut voices) = (false, false, HashSet::new());
        for members in FAMILIES {
            for name in members {
                let dir = self.root.join(name.replace('/', "\\"));
                if !dir.exists() {
                    continue;
                }
                let Ok(live) = fs::read(dir.join("index")) else {
                    st.state = State::Missing;
                    return Ok(st);
                };
                let marker: Option<Marker> = fs::read(dir.join(MARKER)).ok().and_then(|b| serde_json::from_slice(&b).ok());
                ours |= marker.is_some() || !our_blobs(&dir).is_empty();
                if !index::is_patched(&live)? {
                    continue;
                }
                patched = true;
                st.redirected += index::redirected_entries(&live)?;
                match marker.filter(|m| m.index_sha1 == sha1_hex(&live)) {
                    Some(m) => {
                        st.stale |= m.sources.iter().any(|(rel, s)| stat(&self.root.join(rel)).ok().as_ref() != Some(s));
                        voices.insert((m.voice_id, m.voice_date));
                    }
                    None => {
                        voices.insert(Default::default());
                    }
                }
            }
        }
        st.state = if patched { State::Patched } else if ours { State::Reverted } else { State::Original };
        // one known voice version across all patched packages, or unknown
        st.voice = if voices.len() == 1 { voices.into_iter().next().filter(|v| !v.0.is_empty()) } else { None };
        Ok(st)
    }

    /// Scan the referenced audio bundles of every package and list those whose clips differ from JP.
    fn plan(&self, pkgs: &[Pkg], voices: &Voices, progress: Progress) -> Result<Vec<Item>> {
        // (pkg, blob) -> referenced slots, from the original main tables
        let mut blobs: BTreeMap<(usize, u16), Vec<u16>> = BTreeMap::new();
        for (pi, p) in pkgs.iter().enumerate() {
            for (b, s) in index::slots(&p.orig)? {
                blobs.entry((pi, b)).or_default().push(s);
            }
        }
        let total = blobs.len() as u64;
        let done = AtomicU64::new(0);
        let items: Vec<Vec<Item>> = blobs
            .par_iter()
            .map(|(&(pi, b), slots)| -> Result<Vec<Item>> {
                let path = pkgs[pi].dir.join(format!("{b:04}.blob"));
                let data = fs::read(&path).with_context(|| path.display().to_string())?;
                let table = blob::read_table(&path)?;
                let mut out = Vec::new();
                for &s in slots {
                    let raw = table.get(s as usize).and_then(|&(a, z)| data.get(a as usize..z as usize));
                    let Some(raw) = raw.filter(|r| r.starts_with(b"UnityFS\0")) else { continue };
                    let Some(mut a) = audio::Audio::parse(raw).with_context(|| format!("{}#{s}", path.display()))? else { continue };
                    let jp: Vec<(String, String)> = a
                        .hashes()?
                        .into_iter()
                        .filter_map(|(p, tw)| voices.lookup(&a.name, &p).filter(|jp| *jp != tw).map(|jp| (p, jp.to_string())))
                        .collect();
                    if !jp.is_empty() {
                        out.push(Item { pkg: pi, blob: b, slot: s, jp });
                    }
                }
                progress(Step::Scan, done.fetch_add(1, Ordering::Relaxed) + 1, total);
                Ok(out)
            })
            .collect::<Result<_>>()?;
        Ok(items.into_iter().flatten().collect())
    }

    /// -> (bundles rebuilt, clips replaced)
    pub fn apply(&self, voices: &Voices, progress: Progress) -> Result<(usize, usize)> {
        if self.game_running() {
            bail!("請先關閉遊戲");
        }
        let pkgs = self.pkgs()?;
        let items = self.plan(&pkgs, voices, progress)?;
        let need: HashSet<String> = items.iter().flat_map(|i| i.jp.iter().map(|(_, h)| h.clone())).collect();
        voices.fetch(&need, &|d, t| progress(Step::Download, d, t))?;

        // one blob number per family, not the one its live indexes use
        let number: Vec<u16> = (0..FAMILIES.len())
            .map(|f| -> Result<u16> {
                let mut used = HashSet::new();
                for p in pkgs.iter().filter(|p| p.family == f && p.patched) {
                    used.extend(index::redirect_blobs(&p.live)?);
                }
                Ok(if used.contains(&BLOB_NO) { BLOB_NO + 1 } else { BLOB_NO })
            })
            .collect::<Result<_>>()?;

        // rebuild in parallel, appending each bundle to its package's new blob as it is done
        let mut counts = vec![0; pkgs.len()];
        items.iter().for_each(|i| counts[i.pkg] += 1);
        let mut out: Vec<Option<Mutex<(blob::Writer, HashMap<Key, Key>)>>> = Vec::new();
        for (pi, p) in pkgs.iter().enumerate() {
            out.push(match counts[pi] {
                0 => None,
                n => Some(Mutex::new((blob::Writer::create(p.dir.join(format!("{:04}.blob", number[p.family])), n)?, HashMap::new()))),
            });
        }
        let done = AtomicU64::new(0);
        items.par_iter().try_for_each(|it| -> Result<()> {
            let p = &pkgs[it.pkg];
            let src = p.dir.join(format!("{:04}.blob", it.blob));
            let raw = read_slot(&src, it.slot)?;
            let data: Vec<Vec<u8>> = it.jp.iter().map(|(_, h)| voices.clip(h)).collect::<Result<_>>()?;
            let mut jp = HashMap::new();
            for ((path, h), d) in it.jp.iter().zip(&data) {
                let fields = voices.fields(h).ok_or_else(|| anyhow!("manifest 缺少 {h}"))?;
                jp.insert(path.clone(), audio::Replacement { fields, data: d });
            }
            let ctx = || format!("{}\\{:04}.blob#{}", p.name, it.blob, it.slot);
            let audio = audio::Audio::parse(&raw).with_context(ctx)?.ok_or_else(|| anyhow!("{}: 沒有語音（遊戲檔案在掃描後被改動？）", ctx()))?;
            let (bundle, n) = audio.replace(&jp).with_context(ctx)?;
            if n < it.jp.len() {
                bail!("{}: 只換了 {n}/{} 段語音", ctx(), it.jp.len());
            }
            let mut w = out[it.pkg].as_ref().unwrap().lock().unwrap();
            let slot = w.0.append(&bundle)?;
            w.1.insert((it.blob, it.slot), (number[p.family], slot));
            progress(Step::Rebuild, done.fetch_add(1, Ordering::Relaxed) + 1, items.len() as u64);
            Ok(())
        })?;
        let mut out: Vec<Option<(blob::Writer, HashMap<Key, Key>)>> = out.into_iter().map(|m| m.map(|m| m.into_inner().unwrap())).collect();
        for w in out.iter_mut().flatten() {
            w.0.finish()?;
        }

        // commit: index.bak -> new blobs -> indexes (a family's indexes point into each other's blobs)
        // -> markers -> old blobs. An interruption leaves every index pointing at existing files.
        progress(Step::Commit, 0, 1);
        let empty = HashMap::new();
        let red = |pi: usize| out[pi].as_ref().map_or(&empty, |w| &w.1);
        let mut new_index = Vec::new();
        for p in &pkgs {
            let mut fam: [&HashMap<Key, Key>; 2] = [&empty, &empty];
            for (qi, q) in pkgs.iter().enumerate().filter(|(_, q)| q.family == p.family) {
                fam[q.member] = red(qi);
            }
            new_index.push(index::patch(&p.orig, fam).with_context(|| format!("{}\\index", p.name))?.0);
        }
        for p in pkgs.iter().filter(|p| !p.patched) {
            write_atomic(&p.dir.join("index.bak"), &p.live)?;
        }
        let mut keep = HashSet::new();
        for w in out.iter_mut().flatten() {
            fs::rename(&w.0.tmp, &w.0.path).with_context(|| w.0.path.display().to_string())?;
            keep.insert(w.0.path.clone());
        }
        for (pi, p) in pkgs.iter().enumerate() {
            if new_index[pi] != p.live {
                write_atomic(&p.dir.join("index"), &new_index[pi])?;
            }
            if index::is_patched(&new_index[pi])? {
                let mut sources = BTreeMap::new();
                for it in items.iter().filter(|i| i.pkg == pi) {
                    let rel = format!("{}/{:04}.blob", p.name, it.blob);
                    if !sources.contains_key(&rel) {
                        sources.insert(rel, stat(&p.dir.join(format!("{:04}.blob", it.blob)))?);
                    }
                }
                let m = Marker { voice_id: voices.id().into(), voice_date: voices.latest.date.clone(), index_sha1: sha1_hex(&new_index[pi]), sources };
                write_atomic(&p.dir.join(MARKER), &serde_json::to_vec_pretty(&m)?)?;
            } else {
                let _ = fs::remove_file(p.dir.join(MARKER));
            }
        }
        for p in &pkgs {
            for b in our_blobs(&p.dir).into_iter().filter(|b| !keep.contains(b)) {
                fs::remove_file(&b).with_context(|| b.display().to_string())?;
            }
        }
        voices.prune_cache();
        Ok((items.len(), need.len()))
    }

    pub fn restore(&self) -> Result<()> {
        if self.game_running() {
            bail!("請先關閉遊戲");
        }
        for p in self.pkgs()? {
            if p.patched {
                write_atomic(&p.dir.join("index"), &p.orig)?;
            }
            for b in our_blobs(&p.dir) {
                fs::remove_file(&b).with_context(|| b.display().to_string())?;
            }
            let _ = fs::remove_file(p.dir.join(MARKER));
        }
        Ok(())
    }
}
