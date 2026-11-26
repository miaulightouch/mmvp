//! AudioClip discovery and in-place patching inside one UnityFS bundle.
//!
//! Patching rebuilds only the resource streams that receive JP audio (clips sorted by their old
//! offset, 32-byte aligned, same as tools/patch_bundle.py) and overwrites the AudioClip fields in
//! place. [確認] every AudioClip field we change is fixed-size, so object sizes never change.
use crate::serialized::SerializedFile;
use crate::typetree::{read_object, Val};
use crate::unityfs::{self, Bundle};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;

pub const CLASS_AUDIOCLIP: i32 = 83;
pub const CLASS_ASSETBUNDLE: i32 = 142;
const NODE_SERIALIZED: u32 = 4;
const ALIGN: usize = 32;

pub fn sha1_hex(b: &[u8]) -> String {
    let d = Sha1::digest(b);
    d.iter().map(|x| format!("{x:02x}")).collect()
}

/// Incremental SHA1 (for downloads).
pub struct Sha1Stream(Sha1);
impl Sha1Stream {
    pub fn new() -> Self {
        Sha1Stream(Sha1::new())
    }
    pub fn update(&mut self, b: &[u8]) {
        self.0.update(b);
    }
    pub fn hex(self) -> String {
        self.0.finalize().iter().map(|x| format!("{x:02x}")).collect()
    }
}
impl Default for Sha1Stream {
    fn default() -> Self {
        Self::new()
    }
}

struct Clip {
    node: usize,         // serialized node index
    obj_start: usize,    // object start inside the serialized node
    path: Option<String>,
    stream: String,      // resource node name
    offset: u64,
    size: u64,
    val: Val,
}

struct Parsed {
    name: String,
    clips: Vec<Clip>,
}

/// Parse the serialized nodes of a bundle. Returns None if no node contains AudioClips.
/// Only decompresses as far as the serialized nodes reach.
fn parse(b: &mut Bundle) -> Result<Option<Parsed>> {
    let mut name = String::new();
    let mut clips = Vec::new();
    let mut any = false;
    for ni in 0..b.nodes.len() {
        if b.nodes[ni].flags & NODE_SERIALIZED == 0 {
            continue;
        }
        let data = b.node_data(ni)?;
        let sf = SerializedFile::parse(data).with_context(|| format!("node {}", ni))?;
        if !sf.has_class(CLASS_AUDIOCLIP) {
            continue;
        }
        any = true;
        let mut cont: HashMap<i64, String> = HashMap::new();
        for o in &sf.objects {
            if sf.types[o.type_idx].class_id != CLASS_ASSETBUNDLE {
                continue;
            }
            let v = read_object(&sf.types[o.type_idx].tree, &data[o.start..o.start + o.size], sf.big_endian)?;
            if name.is_empty() {
                name = v.get("m_Name").and_then(Val::str).unwrap_or_default().to_string();
            }
            for pair in v.get("m_Container").and_then(Val::arr).unwrap_or(&[]) {
                let (Some(p), Some(asset)) = (pair.get("first").and_then(Val::str), pair.get("second").and_then(|s| s.get("asset"))) else { continue };
                if asset.get("m_FileID").and_then(Val::int) == Some(0) {
                    if let Some(pid) = asset.get("m_PathID").and_then(Val::int) {
                        cont.entry(pid).or_insert_with(|| p.to_string());
                    }
                }
            }
        }
        for o in &sf.objects {
            if sf.types[o.type_idx].class_id != CLASS_AUDIOCLIP {
                continue;
            }
            let v = read_object(&sf.types[o.type_idx].tree, &data[o.start..o.start + o.size], sf.big_endian)?;
            let res = v.get("m_Resource").ok_or_else(|| anyhow!("AudioClip without m_Resource"))?;
            let src = res.get("m_Source").and_then(Val::str).unwrap_or_default();
            let stream = src.rsplit('/').next().unwrap_or_default().to_string();
            let offset = res.get("m_Offset").and_then(Val::int).unwrap_or(0) as u64;
            let size = res.get("m_Size").and_then(Val::int).unwrap_or(0) as u64;
            clips.push(Clip { node: ni, obj_start: o.start, path: cont.get(&o.path_id).cloned(), stream, offset, size, val: v });
        }
    }
    Ok(if any { Some(Parsed { name, clips }) } else { None })
}

fn clip_bytes<'b>(b: &'b mut Bundle, c: &Clip) -> Result<&'b [u8]> {
    let ri = b.node_index(&c.stream).ok_or_else(|| anyhow!("resource stream {} not inside bundle", c.stream))?;
    let d = b.node_data(ri)?;
    let (s, e) = (c.offset as usize, (c.offset + c.size) as usize);
    if e > d.len() {
        bail!("clip data past end of {}", c.stream);
    }
    Ok(&d[s..e])
}

pub struct ScanResult {
    pub name: String,
    /// container path -> sha1 of the raw clip bytes (first clip per path)
    pub clips: Vec<(String, String)>,
}

/// Hash every AudioClip that has a container path. None = no AudioClips in this bundle.
pub fn scan_bundle(raw: &[u8]) -> Result<Option<ScanResult>> {
    let mut b = Bundle::parse(raw)?;
    let Some(p) = parse(&mut b)? else { return Ok(None) };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in &p.clips {
        let Some(path) = &c.path else { continue };
        if !seen.insert(path.clone()) {
            continue;
        }
        out.push((path.clone(), sha1_hex(clip_bytes(&mut b, c)?)));
    }
    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(ScanResult { name: p.name, clips: out }))
}

pub struct Replacement<'a> {
    pub fields: &'a Map<String, Value>,
    pub data: &'a [u8],
}

fn write_prim(buf: &mut [u8], base: usize, v: &Val, j: &Value) -> Result<()> {
    let w = |buf: &mut [u8], off: usize, bytes: &[u8]| buf[base + off..base + off + bytes.len()].copy_from_slice(bytes);
    match *v {
        Val::Int { off, size, .. } | Val::UInt { off, size, .. } => {
            let x = j.as_i64().or_else(|| j.as_u64().map(|u| u as i64)).or_else(|| j.as_bool().map(|b| b as i64))
                .ok_or_else(|| anyhow!("expected integer, got {j}"))?;
            match size {
                1 => w(buf, off, &[x as u8]),
                2 => w(buf, off, &(x as u16).to_le_bytes()),
                4 => w(buf, off, &(x as u32).to_le_bytes()),
                8 => w(buf, off, &(x as u64).to_le_bytes()),
                _ => bail!("bad int size"),
            }
        }
        Val::Float { off, size, .. } => {
            let x = j.as_f64().ok_or_else(|| anyhow!("expected number, got {j}"))?;
            if size == 4 { w(buf, off, &(x as f32).to_le_bytes()) } else { w(buf, off, &x.to_le_bytes()) }
        }
        Val::Bool { off, .. } => {
            let x = j.as_bool().or_else(|| j.as_i64().map(|i| i != 0)).ok_or_else(|| anyhow!("expected bool, got {j}"))?;
            w(buf, off, &[x as u8]);
        }
        _ => bail!("field is not a primitive"),
    }
    Ok(())
}

/// Replace the clips whose container path is in `need`. -> (new bundle bytes, clips replaced)
pub fn patch_bundle(raw: &[u8], need: &HashMap<String, Replacement>) -> Result<(Vec<u8>, usize)> {
    let mut b = Bundle::parse(raw)?;
    let total = b.nodes.iter().map(|n| (n.offset + n.size) as usize).max().unwrap_or(0);
    b.ensure(total)?;
    let p = parse(&mut b)?.ok_or_else(|| anyhow!("no AudioClips in bundle"))?;
    if !p.clips.iter().any(|c| c.path.as_ref().map_or(false, |x| need.contains_key(x))) {
        bail!("none of the requested clips is in this bundle");
    }
    let mut node_bytes: Vec<Vec<u8>> = (0..b.nodes.len()).map(|i| b.node_data(i).map(|d| d.to_vec())).collect::<Result<_>>()?;

    let mut streams: HashMap<&str, Vec<&Clip>> = HashMap::new();
    for c in &p.clips {
        streams.entry(c.stream.as_str()).or_default().push(c);
    }
    let mut replaced = 0;
    for (stream, mut clips) in streams {
        if !clips.iter().any(|c| c.path.as_ref().map_or(false, |x| need.contains_key(x))) {
            continue; // untouched stream keeps its original bytes and offsets
        }
        let ri = b.node_index(stream).ok_or_else(|| anyhow!("resource stream {stream} not inside bundle"))?;
        clips.sort_by_key(|c| c.offset);
        let old = std::mem::take(&mut node_bytes[ri]);
        let mut buf: Vec<u8> = Vec::with_capacity(old.len());
        for c in clips {
            let rep = c.path.as_ref().and_then(|x| need.get(x));
            let data: &[u8] = match rep {
                Some(r) => r.data,
                None => {
                    let (s, e) = (c.offset as usize, (c.offset + c.size) as usize);
                    old.get(s..e).ok_or_else(|| anyhow!("clip data past end of {stream}"))?
                }
            };
            buf.resize(crate::rd::align_up(buf.len(), ALIGN), 0);
            let new_off = buf.len() as u64;
            buf.extend_from_slice(data);
            let sf = &mut node_bytes[c.node];
            let res = c.val.get("m_Resource").unwrap();
            for (k, x) in [("m_Offset", new_off), ("m_Size", data.len() as u64)] {
                let f = res.get(k).ok_or_else(|| anyhow!("m_Resource.{k} missing"))?;
                write_prim(sf, c.obj_start, f, &Value::from(x))?;
            }
            if let Some(r) = rep {
                for (k, j) in r.fields {
                    let f = c.val.get(k).ok_or_else(|| anyhow!("AudioClip field {k} missing"))?;
                    write_prim(sf, c.obj_start, f, j)?;
                }
                replaced += 1;
            }
        }
        node_bytes[ri] = buf;
    }
    let nodes: Vec<(u32, &str, &[u8])> = b.nodes.iter().zip(&node_bytes).map(|(n, d)| (n.flags, n.name.as_str(), d.as_slice())).collect();
    Ok((unityfs::write(b.version, &b.player_ver, &b.engine_ver, &nodes), replaced))
}
