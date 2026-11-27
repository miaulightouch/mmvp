//! AudioClips of one UnityFS bundle: discovery (name, container path, raw bytes) and replacement.
//!
//! Replacement rebuilds only the resource streams that receive JP audio (clips in their old order,
//! 32-byte aligned) and overwrites AudioClip fields in place: [確認] every field we change is
//! fixed-size, so no object moves and everything except the audio stays byte-identical.
use crate::serialized::SerializedFile;
use crate::typetree::{read_object, Val};
use crate::unityfs::{self, Bundle};
use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};
use std::collections::HashMap;

const AUDIO_CLIP: i32 = 83;
const ASSET_BUNDLE: i32 = 142;
const NODE_SERIALIZED: u32 = 4;
const ALIGN: usize = 32;

struct Clip {
    node: usize,
    obj: usize, // object start inside the serialized node
    path: Option<String>,
    stream: String,
    offset: usize,
    size: usize,
    val: Val,
}

pub struct Audio<'a> {
    bundle: Bundle<'a>,
    /// AssetBundle.m_Name, identical in the JP and TW builds
    pub name: String,
    clips: Vec<Clip>,
}

/// JP replacement for one clip: AudioClip fields to overwrite and the raw audio.
pub struct Replacement<'a> {
    pub fields: &'a Map<String, Value>,
    pub data: &'a [u8],
}

impl<'a> Audio<'a> {
    /// None if the bundle holds no AudioClip. Decompresses only up to the serialized files.
    pub fn parse(raw: &'a [u8]) -> Result<Option<Audio<'a>>> {
        let mut bundle = Bundle::parse(raw)?;
        let (mut name, mut clips) = (String::new(), Vec::new());
        for ni in 0..bundle.nodes.len() {
            if bundle.nodes[ni].flags & NODE_SERIALIZED == 0 {
                continue;
            }
            let data = bundle.node_data(ni)?;
            let sf = SerializedFile::parse(data).with_context(|| format!("node {ni}"))?;
            if !sf.types.iter().any(|t| t.class_id == AUDIO_CLIP) {
                continue;
            }
            let sf = &sf;
            let objects = |class: i32| sf.objects.iter().filter(move |o| sf.types[o.type_idx].class_id == class);
            let read = |o: &crate::serialized::Obj| read_object(&sf.types[o.type_idx].tree, &data[o.start..o.start + o.size]);
            let mut paths: HashMap<i64, String> = HashMap::new();
            for o in objects(ASSET_BUNDLE) {
                let v = read(o)?;
                if name.is_empty() {
                    name = v.get("m_Name").and_then(Val::str).unwrap_or_default().to_string();
                }
                for pair in v.get("m_Container").and_then(Val::arr).unwrap_or_default() {
                    let asset = pair.get("second").and_then(|s| s.get("asset"));
                    let (Some(path), Some(asset)) = (pair.get("first").and_then(Val::str), asset) else { continue };
                    if asset.get("m_FileID").and_then(Val::int) == Some(0) {
                        if let Some(id) = asset.get("m_PathID").and_then(Val::int) {
                            paths.entry(id).or_insert_with(|| path.to_string());
                        }
                    }
                }
            }
            for o in objects(AUDIO_CLIP) {
                let val = read(o)?;
                let res = val.get("m_Resource").ok_or_else(|| anyhow!("AudioClip without m_Resource"))?;
                let src = res.get("m_Source").and_then(Val::str).unwrap_or_default();
                let num = |k| res.get(k).and_then(Val::int).unwrap_or(0) as usize;
                clips.push(Clip {
                    node: ni,
                    obj: o.start,
                    path: paths.get(&o.path_id).cloned(),
                    stream: src.rsplit('/').next().unwrap_or_default().to_string(),
                    offset: num("m_Offset"),
                    size: num("m_Size"),
                    val,
                });
            }
        }
        Ok((!clips.is_empty()).then_some(Audio { bundle, name, clips }))
    }

    fn bytes(&mut self, i: usize) -> Result<&[u8]> {
        let c = &self.clips[i];
        let (s, e) = (c.offset, c.offset + c.size);
        let ri = self.bundle.node_index(&c.stream).ok_or_else(|| anyhow!("stream {} not in bundle", c.stream))?;
        self.bundle.node_data(ri)?.get(s..e).ok_or_else(|| anyhow!("clip past end of stream"))
    }

    /// (container path, SHA1 of the raw audio) of every clip with a path; first clip per path.
    pub fn hashes(&mut self) -> Result<Vec<(String, String)>> {
        let mut out: Vec<(String, String)> = Vec::new();
        for i in 0..self.clips.len() {
            let Some(path) = self.clips[i].path.clone() else { continue };
            if !out.iter().any(|(p, _)| *p == path) {
                out.push((path, crate::sha1_hex(self.bytes(i)?)));
            }
        }
        Ok(out)
    }

    /// Replace the clips whose container path is in `jp`. -> (new bundle, clips replaced)
    pub fn replace(mut self, jp: &HashMap<String, Replacement>) -> Result<(Vec<u8>, usize)> {
        let b = &mut self.bundle;
        let mut nodes: Vec<Vec<u8>> = (0..b.nodes.len()).map(|i| b.node_data(i).map(<[u8]>::to_vec)).collect::<Result<_>>()?;
        let hit = |c: &Clip| c.path.as_ref().is_some_and(|p| jp.contains_key(p));
        let streams: Vec<&str> = {
            let mut s: Vec<&str> = self.clips.iter().filter(|c| hit(c)).map(|c| c.stream.as_str()).collect();
            s.sort();
            s.dedup();
            s
        };
        let mut replaced = 0;
        for stream in streams {
            let ri = b.node_index(stream).ok_or_else(|| anyhow!("stream {stream} not in bundle"))?;
            let old = std::mem::take(&mut nodes[ri]);
            let mut clips: Vec<&Clip> = self.clips.iter().filter(|c| c.stream == stream).collect();
            clips.sort_by_key(|c| c.offset);
            let mut buf = Vec::with_capacity(old.len());
            for c in clips {
                let rep = c.path.as_ref().and_then(|p| jp.get(p));
                let data = match rep {
                    Some(r) => r.data,
                    None => old.get(c.offset..c.offset + c.size).ok_or_else(|| anyhow!("clip past end of {stream}"))?,
                };
                buf.resize(buf.len().next_multiple_of(ALIGN), 0);
                let res = c.val.get("m_Resource").unwrap();
                let sf = &mut nodes[c.node];
                set(sf, c.obj, res.get("m_Offset"), &Value::from(buf.len()))?;
                set(sf, c.obj, res.get("m_Size"), &Value::from(data.len()))?;
                if let Some(r) = rep {
                    for (k, v) in r.fields {
                        set(sf, c.obj, c.val.get(k), v)?;
                    }
                    replaced += 1;
                }
                buf.extend_from_slice(data);
            }
            nodes[ri] = buf;
        }
        let out: Vec<(u32, &str, &[u8])> = b.nodes.iter().zip(&nodes).map(|(n, d)| (n.flags, n.name.as_str(), d.as_slice())).collect();
        Ok((unityfs::write(b, &out), replaced))
    }
}

/// Overwrite one fixed-size primitive field (little-endian) at `base + field offset`.
fn set(buf: &mut [u8], base: usize, field: Option<&Val>, v: &Value) -> Result<()> {
    let field = field.ok_or_else(|| anyhow!("AudioClip field missing"))?;
    let (off, size) = field.loc().ok_or_else(|| anyhow!("AudioClip field is not a primitive"))?;
    let bytes: Vec<u8> = match field {
        Val::Float { .. } => {
            let x = v.as_f64().ok_or_else(|| anyhow!("expected number, got {v}"))?;
            if size == 4 { (x as f32).to_le_bytes().to_vec() } else { x.to_le_bytes().to_vec() }
        }
        _ => {
            let x = v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)).or_else(|| v.as_bool().map(i64::from));
            let x = x.ok_or_else(|| anyhow!("expected integer, got {v}"))?;
            x.to_le_bytes()[..size].to_vec()
        }
    };
    buf[base + off..base + off + size].copy_from_slice(&bytes);
    Ok(())
}
