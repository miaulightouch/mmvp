//! A TW install: status, apply, restore.
//!
//! Apply = scan the referenced audio bundles, download the JP clips that differ, rebuild those bundles
//! into `<pkg>/9876.blob` (or 9877, whichever the live index does not use) and redirect every index
//! location of a rebuilt bundle to its new slot. Original blobs are never touched.
//!
//! State lives in the install itself:
//!  * an index is patched if a location points at 9876 or 9877; its original is `<pkg>/index.bak`
//!    (only ever written from an unpatched index);
//!  * `9876.blob` / `9877.blob` in a package directory are ours, any other blob is a game file;
//!  * `<pkg>/index.mmvp.json` records the voice version and the original blobs used, to tell when
//!    a re-apply is needed.
use super::voices::Voices;
use anyhow::{anyhow, bail, Context, Result};
use mmvp_shared::index::{self, Key, BLOBS};
use mmvp_shared::{audio, blob, blob_no, sha1_hex, FAMILIES};
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
const MARKER: &str = "index.mmvp.json";

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

/// A voice version: the manifest SHA1 and the date it was dumped.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Voice {
    pub id: String,
    pub date: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub state: State,
    /// asset entries pointing at our blobs
    pub redirected: usize,
    /// voice version the install was patched with, if known
    pub voice: Option<Voice>,
    /// original blobs changed since apply (game update that kept the index): re-apply needed
    pub stale: bool,
}

/// What an apply did.
#[derive(Clone, Debug, Serialize)]
pub struct Applied {
    pub bundles: usize,
    pub clips: usize,
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
    family: usize,
    member: usize, // 0 = base, 1 = Starter
    dir: PathBuf,
    live: Vec<u8>,
    orig: Vec<u8>,
    patched: bool,
}

/// A bundle to rebuild: slot `slot` of `<blob>.blob`, with the JP clip (SHA1) per container path.
struct Item {
    blob: u16,
    slot: u16,
    jp: Vec<(String, String)>,
}

/// Redirects (old location -> new location) per package, as `[family][member]`.
type Redirects = [[HashMap<Key, Key>; 2]; 2];

pub struct Install {
    pub root: PathBuf,
}

fn stat(p: &Path) -> Result<(u64, u64)> {
    let m = fs::metadata(p).with_context(|| p.display().to_string())?;
    Ok((m.len(), m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos() as u64))
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".mmvp.tmp");
    fs::write(&tmp, data).with_context(|| path.display().to_string())?;
    fs::rename(&tmp, path).with_context(|| path.display().to_string())
}

fn blob_path(dir: &Path, blob: u16) -> PathBuf {
    dir.join(format!("{blob:04}.blob"))
}

fn read_slot(path: &Path, slot: u16) -> Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    let (s, e) = *blob::table(&mut f)?.get(slot as usize).ok_or_else(|| anyhow!("no slot {slot}"))?;
    f.seek(SeekFrom::Start(s))?;
    let mut buf = vec![0; (e - s) as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Our blobs (9876 / 9877) in a package directory, with their numbers.
fn our_blobs(dir: &Path) -> Vec<(u16, PathBuf)> {
    let entries = fs::read_dir(dir).into_iter().flatten().flatten();
    let ours = |e: &fs::DirEntry| blob_no(&e.file_name().to_string_lossy()).filter(|&b| index::is_ours(b));
    entries.filter_map(|e| Some((ours(&e)?, e.path()))).collect()
}

/// The blob number a family's live indexes do not use.
fn spare_blob(pkgs: &[Pkg], family: usize) -> Result<u16> {
    let mut used = HashSet::new();
    for p in pkgs.iter().filter(|p| p.family == family && p.patched) {
        used.extend(index::redirect_blobs(&p.live)?);
    }
    Ok(if used.contains(&BLOBS[0]) { BLOBS[1] } else { BLOBS[0] })
}

/// One bundle with its JP clips in place.
fn rebuild_bundle(p: &Pkg, it: &Item, voices: &Voices) -> Result<Vec<u8>> {
    let raw = read_slot(&blob_path(&p.dir, it.blob), it.slot)?;
    let mut jp = HashMap::new();
    for (path, h) in &it.jp {
        let fields = voices.fields(h).ok_or_else(|| anyhow!("manifest 缺少 {h}"))?;
        jp.insert(path.clone(), audio::Replacement { fields, data: voices.clip(h)? });
    }
    let audio = audio::Audio::parse(&raw)?.ok_or_else(|| anyhow!("沒有語音（遊戲檔案在掃描後被改動？）"))?;
    let (bundle, n) = audio.replace(&jp)?;
    if n < it.jp.len() {
        bail!("只換了 {n}/{} 段語音", it.jp.len());
    }
    Ok(bundle)
}

impl Install {
    /// `root` is the folder that holds AssetPackages (…\MabinogiMobile_TW\appdata).
    pub fn open(root: &Path) -> Result<Install> {
        let ap = root.join("AssetPackages");
        if !ap.join("index").exists() && !ap.join("index.bak").exists() {
            bail!("{} 不是瑪奇 Mobile 的遊戲目錄（找不到 AssetPackages\\index）", root.display());
        }
        Ok(Install { root: root.to_path_buf() })
    }

    /// True if this install's game is running: Windows refuses write access to a running exe.
    pub fn game_running(&self) -> bool {
        let exe = fs::OpenOptions::new().write(true).open(self.root.join(GAME_EXE));
        matches!(exe, Err(e) if e.raw_os_error() == Some(32)) // ERROR_SHARING_VIOLATION
    }

    /// (family, member, name, directory) of every package directory that exists.
    fn dirs(&self) -> Vec<(usize, usize, &'static str, PathBuf)> {
        let mut out = Vec::new();
        for (family, members) in FAMILIES.iter().enumerate() {
            for (member, &name) in members.iter().enumerate() {
                out.push((family, member, name, self.root.join(name)));
            }
        }
        out.retain(|d| d.3.exists());
        out
    }

    fn pkgs(&self) -> Result<Vec<Pkg>> {
        let mut out = Vec::new();
        for (family, member, name, dir) in self.dirs() {
            let live = fs::read(dir.join("index")).with_context(|| format!("{name}/index 不存在（launcher 正在更新？）"))?;
            let patched = index::is_patched(&live).with_context(|| format!("{name}/index"))?;
            let orig = if patched {
                let bak = fs::read(dir.join("index.bak")).with_context(|| format!("{name}/index.bak 不存在，請用 launcher 修復遊戲"))?;
                if index::is_patched(&bak)? {
                    bail!("{name}/index.bak 不是原始檔，請用 launcher 修復遊戲");
                }
                bak
            } else {
                live.clone()
            };
            out.push(Pkg { name, family, member, dir, live, orig, patched });
        }
        Ok(out)
    }

    pub fn status(&self) -> Result<Status> {
        let mut st = Status { state: State::Original, redirected: 0, voice: None, stale: false };
        let mut ours = false;
        let mut voices = Vec::new(); // the voice version of each patched package, None if unknown
        for (_, _, _, dir) in self.dirs() {
            let Ok(live) = fs::read(dir.join("index")) else {
                st.state = State::Missing;
                return Ok(st);
            };
            let marker: Option<Marker> = fs::read(dir.join(MARKER)).ok().and_then(|b| serde_json::from_slice(&b).ok());
            ours |= marker.is_some() || !our_blobs(&dir).is_empty();
            if !index::is_patched(&live)? {
                continue;
            }
            st.redirected += index::redirected_entries(&live)?;
            let marker = marker.filter(|m| m.index_sha1 == sha1_hex(&live));
            if let Some(m) = &marker {
                st.stale |= m.sources.iter().any(|(rel, s)| stat(&self.root.join(rel)).ok().as_ref() != Some(s));
            }
            voices.push(marker.map(|m| Voice { id: m.voice_id, date: m.voice_date }));
        }
        st.state = if !voices.is_empty() { State::Patched } else if ours { State::Reverted } else { State::Original };
        voices.dedup();
        if let [voice] = voices.as_slice() {
            st.voice = voice.clone(); // every patched package has the same version
        }
        Ok(st)
    }

    /// Scan the referenced audio bundles of every package; -> per package, those whose clips differ from JP.
    fn plan(&self, pkgs: &[Pkg], voices: &Voices, progress: Progress) -> Result<Vec<Vec<Item>>> {
        // (pkg, blob) -> referenced slots, from the original main tables
        let mut blobs: BTreeMap<(usize, u16), Vec<u16>> = BTreeMap::new();
        for (pi, p) in pkgs.iter().enumerate() {
            for (b, s) in index::slots(&p.orig)? {
                blobs.entry((pi, b)).or_default().push(s);
            }
        }
        let total = blobs.len() as u64;
        let done = AtomicU64::new(0);
        let scan = |(&(pi, b), slots): (&(usize, u16), &Vec<u16>)| -> Result<(usize, Vec<Item>)> {
            let path = blob_path(&pkgs[pi].dir, b);
            let data = fs::read(&path).with_context(|| path.display().to_string())?;
            let table = blob::table(&mut &data[..]).with_context(|| path.display().to_string())?;
            let mut out = Vec::new();
            for &s in slots {
                let Some(raw) = table.get(s as usize).and_then(|&(a, z)| data.get(a as usize..z as usize)) else { continue };
                let Some(mut a) = audio::Audio::parse(raw).with_context(|| format!("{}#{s}", path.display()))? else { continue };
                let tw = a.hashes()?;
                let differs = |(p, tw): (String, String)| voices.lookup(&a.name, &p).filter(|jp| *jp != tw).map(|jp| (p, jp.to_string()));
                let jp: Vec<(String, String)> = tw.into_iter().filter_map(differs).collect();
                if !jp.is_empty() {
                    out.push(Item { blob: b, slot: s, jp });
                }
            }
            progress(Step::Scan, done.fetch_add(1, Ordering::Relaxed) + 1, total);
            Ok((pi, out))
        };
        let mut items: Vec<Vec<Item>> = pkgs.iter().map(|_| Vec::new()).collect();
        for (pi, found) in blobs.par_iter().map(scan).collect::<Result<Vec<_>>>()? {
            items[pi].extend(found);
        }
        Ok(items)
    }

    /// Rebuild every item into its package's blob `number[family]`, in parallel, each bundle appended
    /// as it is done.
    fn rebuild(&self, pkgs: &[Pkg], items: &[Vec<Item>], number: [u16; 2], voices: &Voices, progress: Progress) -> Result<Redirects> {
        let total = items.iter().map(Vec::len).sum::<usize>() as u64;
        let done = AtomicU64::new(0);
        let mut redirects = Redirects::default();
        for (p, items) in pkgs.iter().zip(items).filter(|(_, items)| !items.is_empty()) {
            let no = number[p.family];
            let writer = Mutex::new(blob::Writer::create(blob_path(&p.dir, no), items.len())?);
            let one = |it: &Item| -> Result<(Key, Key)> {
                let bundle = rebuild_bundle(p, it, voices).with_context(|| format!("{}/{:04}.blob#{}", p.name, it.blob, it.slot))?;
                let slot = writer.lock().unwrap().append(&bundle)?;
                progress(Step::Rebuild, done.fetch_add(1, Ordering::Relaxed) + 1, total);
                Ok(((it.blob, it.slot), (no, slot)))
            };
            redirects[p.family][p.member] = items.par_iter().map(one).collect::<Result<_>>()?;
            writer.into_inner().unwrap().finish()?;
        }
        Ok(redirects)
    }

    /// index.bak -> indexes (a family's indexes point into each other's blobs) -> markers -> old blobs.
    /// The new blobs are already in place, so an interruption leaves every index pointing at existing files.
    fn commit(&self, pkgs: &[Pkg], items: &[Vec<Item>], number: [u16; 2], redirects: &Redirects, voices: &Voices) -> Result<()> {
        let mut new_index = Vec::new();
        for p in pkgs {
            new_index.push(index::patch(&p.orig, &redirects[p.family]).with_context(|| format!("{}/index", p.name))?);
        }
        for p in pkgs.iter().filter(|p| !p.patched) {
            write_atomic(&p.dir.join("index.bak"), &p.live)?;
        }
        for ((p, (new, changed)), items) in pkgs.iter().zip(&new_index).zip(items) {
            if *new != p.live {
                write_atomic(&p.dir.join("index"), new)?;
            }
            if *changed == 0 {
                let _ = fs::remove_file(p.dir.join(MARKER));
                continue;
            }
            let mut sources = BTreeMap::new();
            for it in items {
                sources.insert(format!("{}/{:04}.blob", p.name, it.blob), stat(&blob_path(&p.dir, it.blob))?);
            }
            let latest = &voices.latest;
            let m = Marker { voice_id: latest.manifest_sha1.clone(), voice_date: latest.date.clone(), index_sha1: sha1_hex(new), sources };
            write_atomic(&p.dir.join(MARKER), &serde_json::to_vec_pretty(&m)?)?;
        }
        for (p, items) in pkgs.iter().zip(items) {
            for (no, path) in our_blobs(&p.dir) {
                if items.is_empty() || no != number[p.family] {
                    fs::remove_file(&path).with_context(|| path.display().to_string())?;
                }
            }
        }
        Ok(())
    }

    pub fn apply(&self, voices: &Voices, progress: Progress) -> Result<Applied> {
        if self.game_running() {
            bail!("請先關閉遊戲");
        }
        let pkgs = self.pkgs()?;
        let items = self.plan(&pkgs, voices, progress)?;
        let need: HashSet<String> = items.iter().flatten().flat_map(|i| i.jp.iter().map(|(_, h)| h.clone())).collect();
        voices.fetch(&need, &|d, t| progress(Step::Download, d, t))?;
        let number = [spare_blob(&pkgs, 0)?, spare_blob(&pkgs, 1)?];
        let redirects = self.rebuild(&pkgs, &items, number, voices, progress)?;
        progress(Step::Commit, 0, 1);
        self.commit(&pkgs, &items, number, &redirects, voices)?;
        voices.prune_cache();
        Ok(Applied { bundles: items.iter().map(Vec::len).sum(), clips: need.len() })
    }

    pub fn restore(&self) -> Result<()> {
        if self.game_running() {
            bail!("請先關閉遊戲");
        }
        for p in self.pkgs()? {
            if p.patched {
                write_atomic(&p.dir.join("index"), &p.orig)?;
            }
            for (_, path) in our_blobs(&p.dir) {
                fs::remove_file(&path).with_context(|| path.display().to_string())?;
            }
            let _ = fs::remove_file(p.dir.join(MARKER));
        }
        Ok(())
    }
}
