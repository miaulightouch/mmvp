//! JP client -> voice site directory (see mmvp_shared::site).
use anyhow::{Context, Result};
use mmvp_shared::audio::Audio;
use mmvp_shared::site::{Clip, Latest, Manifest, LATEST, MANIFEST, VERSION};
use mmvp_shared::{blob, index, sha1_hex, FAMILIES};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const HEADERS: &str = include_str!("../site/_headers");
const LANDING: &str = include_str!("../site/index.html");
/// Cloudflare Pages: files per deployment.
const MAX_FILES: usize = 20_000;

/// Voices of one blob: references (bundle name, path, sha1) and the first payload per sha1.
type BlobVoices = (Vec<(String, String, String)>, Vec<(String, Map<String, Value>, Vec<u8>)>);

/// `slots`: the slots the package index references, sorted; the others are leftovers of older
/// versions of a bundle and must not be dumped.
fn dump_blob(path: &Path, slots: &[u16]) -> Result<BlobVoices> {
    let data = fs::read(path).with_context(|| path.display().to_string())?;
    let (mut refs, mut clips): BlobVoices = Default::default();
    let table = blob::table(&mut &data[..]).with_context(|| path.display().to_string())?;
    for &i in slots {
        let Some(&(s, e)) = table.get(i as usize) else { anyhow::bail!("{}: index references missing slot {i}", path.display()) };
        let Some(mut audio) = Audio::parse(&data[s as usize..e as usize]).with_context(|| format!("{}#{i}", path.display()))? else { continue };
        for (p, fields, bytes) in audio.voices()? {
            let h = sha1_hex(&bytes);
            if !clips.iter().any(|c| c.0 == h) {
                clips.push((h.clone(), fields, bytes));
            }
            refs.push((audio.name.clone(), p, h));
        }
    }
    Ok((refs, clips))
}

/// Dump `jp_root` into `out/<date>` (`date` = YYYY-MM-DD, shown to users as the voice version);
/// -> that directory.
pub fn dump(jp_root: &Path, out: &Path, date: &str, repo: Option<&str>) -> Result<PathBuf> {
    // the JP launcher deletes index files while it updates: never dump a half-written install
    if !jp_root.join("AssetPackages").join("index").exists() {
        anyhow::bail!("{}\\AssetPackages\\index missing: is the JP launcher updating?", jp_root.display());
    }
    let t0 = Instant::now();
    // only the slots each package index references, like the patcher (blob -> sorted slots)
    let mut blobs: Vec<(PathBuf, Vec<u16>)> = Vec::new();
    for pkg in FAMILIES.iter().flatten() {
        let dir = jp_root.join(pkg);
        if !dir.exists() {
            continue;
        }
        let idx = fs::read(dir.join("index")).with_context(|| format!("{pkg}/index missing: is the JP launcher updating?"))?;
        let mut by_blob: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
        for (b, s) in index::slots(&idx).with_context(|| format!("{pkg}/index"))? {
            by_blob.entry(b).or_default().push(s);
        }
        for (b, mut slots) in by_blob {
            slots.sort();
            blobs.push((dir.join(format!("{b:04}.blob")), slots));
        }
    }
    // parallel, then merged in blob order: the output only depends on the input
    let per_blob: Vec<BlobVoices> = blobs.par_iter().map(|(p, slots)| dump_blob(p, slots)).collect::<Result<_>>()?;

    let mut m = Manifest { version: VERSION, clips: BTreeMap::new(), default: BTreeMap::new(), bundles: BTreeMap::new() };
    let mut votes: HashMap<String, Vec<(String, usize)>> = HashMap::new(); // path -> (sha1, count) in first-seen order
    let mut payloads: Vec<(String, Vec<u8>)> = Vec::new();
    for (refs, clips) in per_blob {
        for (h, fields, bytes) in clips {
            if !m.clips.contains_key(&h) {
                m.clips.insert(h.clone(), Clip { size: bytes.len() as u64, fields });
                payloads.push((h, bytes));
            }
        }
        for (name, path, h) in refs {
            let v = votes.entry(path.clone()).or_default();
            match v.iter_mut().find(|(x, _)| *x == h) {
                Some(e) => e.1 += 1,
                None => v.push((h.clone(), 1)),
            }
            m.bundles.entry(name).or_default().insert(path, h);
        }
    }
    for (path, v) in votes {
        // the most used variant; ties go to the first seen
        let best = v.iter().fold(&v[0], |b, x| if x.1 > b.1 { x } else { b });
        m.default.insert(path, best.0.clone());
    }
    if m.clips.len() + 4 > MAX_FILES {
        anyhow::bail!("{} clips: Cloudflare Pages allows at most {MAX_FILES} files per deployment", m.clips.len());
    }

    let site = out.join(date);
    let _ = fs::remove_dir_all(&site);
    fs::create_dir_all(site.join("clips"))?;
    let bytes: u64 = payloads.iter().map(|p| p.1.len() as u64).sum();
    for (h, data) in &payloads {
        fs::write(site.join("clips").join(h), data)?;
    }
    let raw = serde_json::to_vec(&m)?;
    fs::write(site.join(MANIFEST), &raw)?;
    let latest = Latest { version: VERSION, date: date.into(), manifest_sha1: sha1_hex(&raw), clips: m.clips.len(), bytes };
    fs::write(site.join(LATEST), serde_json::to_vec_pretty(&latest)?)?;
    fs::write(site.join("_headers"), HEADERS)?;
    if let Some(repo) = repo {
        let html = LANDING
            .replace("{{REPO_URL}}", &format!("https://github.com/{}", repo.trim_matches('/')))
            .replace("{{DATE}}", &latest.date)
            .replace("{{CLIPS}}", &latest.clips.to_string())
            .replace("{{MB}}", &format!("{:.0}", bytes as f64 / 1_048_576.0))
            .replace("{{ID}}", &latest.manifest_sha1);
        fs::write(site.join("index.html"), html)?;
    }
    eprintln!(
        "{} blobs, {} paths, {} clips ({:.1} MB), {} bundles in {:.0}s -> {}\nvoice version {}",
        blobs.len(), m.default.len(), m.clips.len(), bytes as f64 / 1_048_576.0, m.bundles.len(), t0.elapsed().as_secs_f64(), site.display(), latest.manifest_sha1
    );
    Ok(site)
}
