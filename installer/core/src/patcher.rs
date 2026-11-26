//! Scan (cached) -> plan -> parallel rebuild straight into <pkg>/<N>.blob -> safe commit.
//!
//! Stateless: everything is derived from the install itself.
//!  * an index is "ours" if some entry points at a blob >= 5000 (index::is_patched);
//!  * the original index of a patched package is <pkg>/index.bak (only ever written from an original);
//!  * every NNNN.blob with NNNN >= 5000 in a package directory is ours.
use crate::audio::{self, Replacement};
use crate::blob::{self, BlobWriter};
use crate::index::{self, Key, REDIRECT_BASE};
use crate::voices::Voices;
use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Instant, UNIX_EPOCH};

pub const PACKAGES: &[&str] = &["AssetPackages", "AssetPackages/Starter", "ResourcesPackage", "ResourcesPackage/Starter", "PlatformResourcePackages"];
pub const DEFAULT_BLOB_NO: u16 = 9876;
pub const GAME_EXE: &str = "MabinogiMobile.exe";
const SCAN_VERSION: u32 = 1;

pub enum Event<'a> {
    Stage(&'a str),
    Progress { stage: &'a str, done: usize, total: usize },
    Warn(String),
    Info(String),
}
pub type Report<'a> = &'a (dyn Fn(Event) + Sync);

// ------------------------------------------------------------------ install layout
fn pkg_dir(root: &Path, pkg: &str) -> PathBuf {
    root.join(pkg.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn blob_number(p: &Path) -> Option<u16> {
    let name = p.file_name()?.to_str()?;
    let stem = name.strip_suffix(".blob")?;
    if stem.is_empty() || !stem.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}

/// Original blobs as "pkg/NNNN.blob", sorted.
pub fn list_blobs(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for pkg in PACKAGES {
        let d = pkg_dir(root, pkg);
        let Ok(rd) = fs::read_dir(&d) else { continue };
        let mut v: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| blob_number(&e.path()).filter(|&n| n < REDIRECT_BASE).map(|_| format!("{pkg}/{}", e.file_name().to_string_lossy())))
            .collect();
        v.sort();
        out.extend(v);
    }
    if out.is_empty() {
        bail!("{} 不是瑪奇 Mobile 的安裝目錄（找不到 AssetPackages/*.blob）", root.display());
    }
    Ok(out)
}

fn our_blobs(dir: &Path) -> Vec<(u16, PathBuf)> {
    fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| blob_number(&e.path()).filter(|&n| n >= REDIRECT_BASE).map(|n| (n, e.path()))).collect()).unwrap_or_default()
}

fn rel_pkg(rel: &str) -> &str {
    rel.rsplit_once('/').map(|x| x.0).unwrap_or("")
}

fn rel_blob_no(rel: &str) -> u16 {
    blob_number(Path::new(rel)).unwrap_or(0)
}

/// True if the game of *this* install is running: Windows refuses write access to a running exe.
/// Opening for write changes nothing (no truncate, no write).
pub fn game_running(root: &Path) -> bool {
    let exe = root.join(GAME_EXE);
    match fs::OpenOptions::new().write(true).open(&exe) {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(32), // ERROR_SHARING_VIOLATION; missing file / no access: not "running"
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = PathBuf::from(format!("{}.mmvoice.tmp", path.display()));
    fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

// ------------------------------------------------------------------ package state
pub struct PkgIndex {
    pub pkg: String,
    pub live: Vec<u8>,
    pub live_patched: bool,
    /// original index: live if not patched, else index.bak
    pub orig: Vec<u8>,
}

pub fn read_pkg_index(root: &Path, pkg: &str) -> Result<PkgIndex> {
    let dir = pkg_dir(root, pkg);
    let live = fs::read(dir.join("index")).with_context(|| format!("{pkg}/index 不存在（launcher 正在更新或修復？）"))?;
    let live_patched = index::is_patched(&live).with_context(|| format!("{pkg}/index"))?;
    let orig = if live_patched {
        let bak = fs::read(dir.join("index.bak")).with_context(|| format!("{pkg}/index 已被修改，但找不到原始備份 index.bak；請用 launcher 修復"))?;
        if index::is_patched(&bak)? {
            bail!("{pkg}/index.bak 不是原始 index；請用 launcher 修復");
        }
        bak
    } else {
        live.clone()
    };
    Ok(PkgIndex { pkg: pkg.to_string(), live, live_patched, orig })
}

/// `<pkg>/index.mmvoice.json`, written next to the index on every apply. The index content stays the
/// source of truth for "is it patched"; the marker only answers "patched with what, and is it still
/// current": which pack, which index we wrote, and the size/mtime of every original blob whose
/// bundles we redirected (a game update that rewrites one of them makes our copy stale).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Marker {
    pub version: u32,
    pub pack_id: String,
    pub pack_created: String,
    pub app_version: String,
    pub orig_index_sha1: String,
    pub index_sha1: String,
    pub blob: String,
    /// "pkg/NNNN.blob" -> (size, mtime_ns)
    pub sources: BTreeMap<String, (u64, u64)>,
}
const MARKER: &str = "index.mmvoice.json";

fn read_marker(dir: &Path) -> Option<Marker> {
    fs::read(dir.join(MARKER)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PkgState {
    /// untouched original
    Original,
    /// patched with `blob`. `pack_id` None = no matching marker (unknown pack: re-apply to be sure).
    /// `stale` = original blobs that changed since apply (game update kept the index): re-apply.
    Patched { blob: u16, pack_id: Option<String>, pack_created: Option<String>, stale: Vec<String> },
    /// our blob / marker exists but the index is an original again (game update / launcher repair)
    Reverted,
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct PkgStatus {
    pub pkg: String,
    pub state: PkgState,
    pub entries_redirected: usize,
}

pub fn status(root: &Path) -> Result<Vec<PkgStatus>> {
    list_blobs(root)?;
    let mut out = Vec::new();
    for pkg in PACKAGES {
        let dir = pkg_dir(root, pkg);
        if !dir.exists() {
            continue;
        }
        let Ok(live) = fs::read(dir.join("index")) else {
            out.push(PkgStatus { pkg: pkg.to_string(), state: PkgState::Missing, entries_redirected: 0 });
            continue;
        };
        let marker = read_marker(&dir);
        let (state, n) = if index::is_patched(&live)? {
            let used = index::redirect_blobs(&live)?;
            let blob = used.iter().copied().max().unwrap_or(0);
            let n = live[8..].chunks_exact(4).take(u32::from_le_bytes(live[4..8].try_into().unwrap()) as usize)
                .filter(|c| u16::from_le_bytes([c[0], c[1]]) & index::MASK >= REDIRECT_BASE).count();
            let m = marker.filter(|m| m.index_sha1 == audio::sha1_hex(&live));
            let stale = m.as_ref().map(|m| {
                m.sources.iter().filter(|(rel, &(size, mtime))| stat(&root.join(rel)).ok() != Some((size, mtime))).map(|(r, _)| r.clone()).collect()
            }).unwrap_or_default();
            (PkgState::Patched { blob, pack_id: m.as_ref().map(|m| m.pack_id.clone()), pack_created: m.map(|m| m.pack_created), stale }, n)
        } else if !our_blobs(&dir).is_empty() || marker.is_some() {
            (PkgState::Reverted, 0)
        } else {
            (PkgState::Original, 0)
        };
        out.push(PkgStatus { pkg: pkg.to_string(), state, entries_redirected: n });
    }
    Ok(out)
}

// ------------------------------------------------------------------ 1. scan (cached)
#[derive(Serialize, Deserialize, Clone)]
pub struct BundleScan {
    pub slot: u32,
    pub name: String,
    pub clips: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BlobScan {
    pub size: u64,
    pub mtime_ns: u64,
    pub bundles: Vec<BundleScan>,
    pub warns: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    root: String,
    blobs: BTreeMap<String, BlobScan>,
}

fn stat(p: &Path) -> Result<(u64, u64)> {
    let m = fs::metadata(p)?;
    let t = m.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    Ok((m.len(), t))
}

fn scan_blob(root: &Path, rel: &str) -> Result<BlobScan> {
    let path = root.join(rel);
    let (size, mtime_ns) = stat(&path)?; // taken before reading: a concurrent rewrite is rescanned next time
    let data = fs::read(&path)?;
    let table = blob::read_table(&path)?;
    let mut bundles = Vec::new();
    let mut warns = Vec::new();
    for (slot, &(s, e)) in table.iter().enumerate() {
        let Some(raw) = data.get(s as usize..e as usize) else {
            warns.push(format!("{rel}#{slot}: slot past end of file"));
            continue;
        };
        if !raw.starts_with(b"UnityFS\0") {
            continue;
        }
        match audio::scan_bundle(raw) {
            Ok(Some(r)) => bundles.push(BundleScan { slot: slot as u32, name: r.name, clips: r.clips }),
            Ok(None) => {}
            Err(e) => warns.push(format!("{rel}#{slot}: {e:#}")),
        }
    }
    Ok(BlobScan { size, mtime_ns, bundles, warns })
}

pub fn scan(root: &Path, cache_path: &Path, rescan: bool, report: Report) -> Result<BTreeMap<String, BlobScan>> {
    report(Event::Stage("scan"));
    let root_key = root.canonicalize().unwrap_or(root.to_path_buf()).display().to_string();
    let mut cache: CacheFile = if rescan { CacheFile::default() } else {
        fs::read(cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    };
    if cache.version != SCAN_VERSION || cache.root != root_key {
        cache = CacheFile { version: SCAN_VERSION, root: root_key, blobs: BTreeMap::new() };
    }
    let blobs = list_blobs(root)?;
    let mut result = BTreeMap::new();
    let mut todo = Vec::new();
    for rel in &blobs {
        let (size, mtime) = stat(&root.join(rel))?;
        match cache.blobs.get(rel) {
            Some(e) if e.size == size && e.mtime_ns == mtime => {
                result.insert(rel.clone(), e.clone());
            }
            _ => todo.push(rel.clone()),
        }
    }
    report(Event::Info(format!("{} 個 blob，{} 個用快取，{} 個要掃描", blobs.len(), result.len(), todo.len())));
    let done = AtomicUsize::new(0);
    let total = todo.len();
    let scanned: Vec<(String, Result<BlobScan>)> = todo
        .par_iter()
        .map(|rel| {
            let r = scan_blob(root, rel);
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            report(Event::Progress { stage: "scan", done: d, total });
            (rel.clone(), r)
        })
        .collect();
    for (rel, r) in scanned {
        result.insert(rel, r?);
    }
    let changed = total > 0 || result.len() != cache.blobs.len();
    cache.blobs = result.clone();
    if changed {
        if let Some(d) = cache_path.parent() {
            fs::create_dir_all(d)?;
        }
        write_atomic(cache_path, &serde_json::to_vec(&cache)?)?;
    }
    for e in result.values() {
        for w in &e.warns {
            report(Event::Warn(format!("略過 bundle {w}")));
        }
    }
    Ok(result)
}

// ------------------------------------------------------------------ 2. plan
pub struct Item {
    pub rel: String,
    pub slot: u16,
    /// container path -> JP sha1
    pub need: Vec<(String, String)>,
}

pub struct Plan {
    pub pkgs: BTreeMap<String, Vec<Item>>,
    pub skipped_unreferenced: usize,
}

impl Plan {
    pub fn bundles(&self) -> usize {
        self.pkgs.values().map(Vec::len).sum()
    }
    pub fn clips(&self) -> usize {
        self.pkgs.values().flatten().map(|i| i.need.len()).sum()
    }
    pub fn blobs(&self) -> usize {
        self.pkgs.values().flatten().map(|i| i.rel.as_str()).collect::<HashSet<_>>().len()
    }
}

pub fn plan(scan: &BTreeMap<String, BlobScan>, voices: &Voices, indexes: &HashMap<String, PkgIndex>) -> Result<Plan> {
    let live: HashMap<&str, HashSet<Key>> = indexes.iter().map(|(k, v)| Ok((k.as_str(), index::slots(&v.orig)?))).collect::<Result<_>>()?;
    let mut pkgs: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    let mut skipped = 0;
    for (rel, e) in scan {
        let pkg = rel_pkg(rel);
        let no = rel_blob_no(rel);
        for b in &e.bundles {
            if !live.get(pkg).map_or(false, |s| s.contains(&(no, b.slot as u16))) {
                skipped += 1;
                continue;
            }
            let need: Vec<(String, String)> = b.clips.iter()
                .filter_map(|(p, cur)| voices.manifest.lookup(&b.name, p).filter(|h| h != cur).map(|h| (p.clone(), h.to_string())))
                .collect();
            if !need.is_empty() {
                pkgs.entry(pkg.to_string()).or_default().push(Item { rel: rel.clone(), slot: b.slot as u16, need });
            }
        }
    }
    for (pkg, items) in &pkgs {
        if items.len() > 0xFFFF {
            bail!("{pkg}: {} 個 bundle 超過單一 blob 上限 65535", items.len());
        }
    }
    Ok(Plan { pkgs, skipped_unreferenced: skipped })
}

// ------------------------------------------------------------------ 3. rebuild
fn rebuild_one(root: &Path, voices: &Voices, it: &Item) -> Result<Vec<u8>> {
    let raw = blob::read_slot(&root.join(&it.rel), it.slot as usize)?;
    let mut datas = Vec::with_capacity(it.need.len());
    for (_, h) in &it.need {
        datas.push(voices.clip(h)?);
    }
    let mut need = HashMap::new();
    for ((p, h), d) in it.need.iter().zip(&datas) {
        let fields = &voices.manifest.clips.get(h).ok_or_else(|| anyhow!("manifest has no fields for {h}"))?.fields;
        need.insert(p.clone(), Replacement { fields, data: d });
    }
    let (new, n) = audio::patch_bundle(&raw, &need).with_context(|| format!("{}#{}", it.rel, it.slot))?;
    if n < it.need.len() {
        bail!("{}#{}: 只換了 {n}/{} 個 clip（blob 在掃描後被改過？）", it.rel, it.slot, it.need.len());
    }
    Ok(new)
}

// ------------------------------------------------------------------ entry points
pub struct PatchSummary {
    pub blobs: usize,
    pub bundles: usize,
    pub clips: usize,
    pub per_pkg: Vec<(String, Option<String>, usize)>,
}

pub struct Options<'a> {
    pub root: &'a Path,
    pub voices: &'a Voices,
    pub cache: &'a Path,
    pub rescan: bool,
    pub dry_run: bool,
    pub blob_no: u16,
}

fn clean_tmp(root: &Path) {
    for pkg in PACKAGES {
        if let Ok(rd) = fs::read_dir(pkg_dir(root, pkg)) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().ends_with(".mmvoice.tmp") {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
}

pub fn patch(o: &Options, report: Report) -> Result<PatchSummary> {
    if !(REDIRECT_BASE..index::MASK).contains(&o.blob_no) {
        bail!("blob 編號必須在 {REDIRECT_BASE}..{}", index::MASK - 1);
    }
    if !o.dry_run && game_running(o.root) {
        bail!("遊戲正在執行，請先關閉 {GAME_EXE}");
    }
    let t0 = Instant::now();
    let scan = scan(o.root, o.cache, o.rescan, report)?;
    let pkgs_present: Vec<&str> = PACKAGES.iter().copied().filter(|p| pkg_dir(o.root, p).join("index").exists() || scan.keys().any(|r| rel_pkg(r) == *p)).collect();
    let mut indexes = HashMap::new();
    for p in &pkgs_present {
        indexes.insert(p.to_string(), read_pkg_index(o.root, p)?);
    }
    report(Event::Stage("plan"));
    let plan = plan(&scan, o.voices, &indexes)?;
    if plan.skipped_unreferenced > 0 {
        report(Event::Info(format!("略過 {} 個 index 沒有引用的音訊 bundle", plan.skipped_unreferenced)));
    }
    report(Event::Info(format!("需要換 JP 語音：{} blob / {} bundle / {} clip", plan.blobs(), plan.bundles(), plan.clips())));
    let mut summary = PatchSummary { blobs: plan.blobs(), bundles: plan.bundles(), clips: plan.clips(), per_pkg: Vec::new() };
    let need: HashSet<String> = plan.pkgs.values().flatten().flat_map(|i| i.need.iter().map(|(_, h)| h.clone())).collect();
    let need_bytes: u64 = need.iter().filter_map(|h| o.voices.manifest.clips.get(h)).map(|c| c.size).sum();
    report(Event::Info(format!("需要 {} 段不重複的 JP 語音（{:.1} MB）", need.len(), need_bytes as f64 / 1048576.0)));
    if o.dry_run {
        return Ok(summary);
    }

    // download what is not cached yet (before touching the game directory)
    report(Event::Stage("download"));
    let got = o.voices.ensure(&need, &|d, t| report(Event::Progress { stage: "download", done: (d >> 10) as usize, total: (t >> 10) as usize }))?;
    if got > 0 {
        report(Event::Info(format!("下載了 {got} 段語音")));
    }
    clean_tmp(o.root);

    // packages to rewrite: those with work, plus patched ones that now need nothing (restore them)
    let todo_pkgs: Vec<String> = indexes.values().filter(|i| plan.pkgs.contains_key(&i.pkg) || i.live_patched).map(|i| i.pkg.clone()).collect();
    let mut numbers = HashMap::new();
    for pkg in &todo_pkgs {
        let used = if indexes[pkg].live_patched { index::redirect_blobs(&indexes[pkg].live)? } else { HashSet::new() };
        numbers.insert(pkg.clone(), if used.contains(&o.blob_no) { o.blob_no + 1 } else { o.blob_no });
    }

    // 3. rebuild, appending into <pkg>/<N>.blob.mmvoice.tmp as results arrive
    report(Event::Stage("rebuild"));
    let mut writers: HashMap<String, BlobWriter> = HashMap::new();
    let mut redirect: HashMap<String, HashMap<Key, Key>> = HashMap::new();
    let result = (|| -> Result<()> {
        for (pkg, items) in &plan.pkgs {
            let path = pkg_dir(o.root, pkg).join(format!("{:04}.blob", numbers[pkg]));
            writers.insert(pkg.clone(), BlobWriter::create(path, items.len())?);
        }
        let tasks: Vec<&Item> = plan.pkgs.values().flatten().collect();
        let total = tasks.len();
        let cancel = AtomicBool::new(false);
        let (tx, rx) = mpsc::sync_channel::<(usize, Result<Vec<u8>>)>(32);
        let mut first_err: Option<anyhow::Error> = None;
        std::thread::scope(|s| {
            let tasks = &tasks;
            let cancel = &cancel;
            s.spawn(move || {
                (0..tasks.len()).into_par_iter().for_each(|i| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let _ = tx.send((i, rebuild_one(o.root, o.voices, tasks[i])));
                });
            });
            let mut done = 0;
            for (i, r) in rx {
                if first_err.is_some() {
                    continue; // drain
                }
                match r.and_then(|bytes| {
                    let it = tasks[i];
                    let pkg = rel_pkg(&it.rel);
                    let ns = writers.get_mut(pkg).unwrap().append(&bytes)?;
                    redirect.entry(pkg.to_string()).or_default().insert((rel_blob_no(&it.rel), it.slot), (numbers[pkg], ns));
                    Ok(())
                }) {
                    Ok(()) => {
                        done += 1;
                        report(Event::Progress { stage: "rebuild", done, total });
                    }
                    Err(e) => {
                        cancel.store(true, Ordering::Relaxed);
                        first_err = Some(e);
                    }
                }
            }
        });
        if let Some(e) = first_err {
            return Err(e);
        }
        for w in writers.values_mut() {
            w.finish()?;
        }
        // every rebuilt bundle must be referenced by its index, and indexes must build cleanly
        for (pkg, red) in &redirect {
            let present = index::slots(&indexes[pkg].orig)?;
            let unref: Vec<_> = red.keys().filter(|k| !present.contains(k)).take(3).collect();
            if !unref.is_empty() {
                bail!("{pkg}: 重建的 bundle 沒有被 index 引用，例如 {unref:?}");
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        for w in writers.values_mut() {
            w.abort();
        }
        return Err(e);
    }

    // 4. commit, package by package: index.bak (originals only) -> new blob in place -> index -> old blobs
    report(Event::Stage("commit"));
    for pkg in &todo_pkgs {
        let ix = &indexes[pkg];
        let dir = pkg_dir(o.root, pkg);
        if !ix.live_patched {
            write_atomic(&dir.join("index.bak"), &ix.live)?;
        }
        let empty = HashMap::new();
        let red = redirect.get(pkg).unwrap_or(&empty);
        let (new_idx, changed) = index::patch(&ix.orig, red)?;
        let mut keep = None;
        if let Some(w) = writers.get(pkg) {
            fs::rename(&w.tmp, &w.path).with_context(|| format!("rename {}", w.path.display()))?;
            keep = Some(w.path.clone());
        }
        write_atomic(&dir.join("index"), &new_idx)?;
        let name = keep.as_ref().map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
        match &name {
            Some(blob) => {
                let mut sources = BTreeMap::new();
                for it in plan.pkgs.get(pkg).into_iter().flatten() {
                    if !sources.contains_key(&it.rel) {
                        sources.insert(it.rel.clone(), stat(&o.root.join(&it.rel))?);
                    }
                }
                let m = Marker {
                    version: 1,
                    pack_id: o.voices.id.clone(),
                    pack_created: o.voices.latest.date.clone(),
                    app_version: env!("CARGO_PKG_VERSION").into(),
                    orig_index_sha1: audio::sha1_hex(&ix.orig),
                    index_sha1: audio::sha1_hex(&new_idx),
                    blob: blob.clone(),
                    sources,
                };
                write_atomic(&dir.join(MARKER), &serde_json::to_vec_pretty(&m)?)?;
            }
            None => {
                let _ = fs::remove_file(dir.join(MARKER));
            }
        }
        for (_, p) in our_blobs(&dir) {
            if Some(&p) != keep.as_ref() {
                fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
            }
        }
        report(Event::Info(format!("{pkg}: {}，{changed} 條 index 重導向", name.clone().unwrap_or_else(|| "還原為原始 index".into()))));
        summary.per_pkg.push((pkg.clone(), name, changed));
    }
    o.voices.prune_cache();
    report(Event::Info(format!("完成，耗時 {:.1} 秒", t0.elapsed().as_secs_f64())));
    Ok(summary)
}

pub fn restore(root: &Path, report: Report) -> Result<()> {
    if game_running(root) {
        bail!("遊戲正在執行，請先關閉 {GAME_EXE}");
    }
    list_blobs(root)?;
    clean_tmp(root);
    for pkg in PACKAGES {
        let dir = pkg_dir(root, pkg);
        if !dir.exists() {
            continue;
        }
        if let Ok(live) = fs::read(dir.join("index")) {
            if index::is_patched(&live)? {
                let ix = read_pkg_index(root, pkg)?;
                write_atomic(&dir.join("index"), &ix.orig)?;
                report(Event::Info(format!("{pkg}: index 已還原")));
            }
        }
        for (_, p) in our_blobs(&dir) {
            fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
            report(Event::Info(format!("{pkg}: 刪除 {}", p.file_name().unwrap().to_string_lossy())));
        }
        let _ = fs::remove_file(dir.join(MARKER));
    }
    Ok(())
}
