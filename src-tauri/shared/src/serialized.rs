//! SerializedFile (format 17..22, as used by Unity 2021.3): header, types with embedded type trees,
//! object table. Only what is needed to read AssetBundle / AudioClip objects.
use crate::common_strings::COMMON_STRINGS;
use crate::rd::Rd;
use anyhow::{bail, Result};

#[derive(Clone, Debug)]
pub struct TNode {
    pub ty: String,
    pub name: String,
    pub level: u8,
    pub byte_size: i32,
    pub meta: u32,
}

pub struct SType {
    pub class_id: i32,
    pub tree: Vec<TNode>,
}

#[derive(Clone, Copy, Debug)]
pub struct Obj {
    pub path_id: i64,
    /// absolute offset of the object data inside the SerializedFile
    pub start: usize,
    pub size: usize,
    pub type_idx: usize,
}

pub struct SerializedFile {
    pub types: Vec<SType>,
    pub objects: Vec<Obj>,
}

fn common(off: u32) -> String {
    match COMMON_STRINGS.binary_search_by_key(&off, |x| x.0) {
        Ok(i) => COMMON_STRINGS[i].1.to_string(),
        Err(_) => off.to_string(),
    }
}

fn str_at(buf: &[u8], v: u32) -> String {
    if v & 0x8000_0000 != 0 {
        return common(v & 0x7FFF_FFFF);
    }
    let s = &buf[(v as usize).min(buf.len())..];
    let e = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..e]).into_owned()
}

fn read_tree(r: &mut Rd, version: u32) -> Result<Vec<TNode>> {
    let n = r.i32()? as usize;
    let sbuf_len = r.i32()? as usize;
    let node_sz = if version >= 19 { 32 } else { 24 };
    let raw = r.take(n * node_sz)?;
    let sbuf = r.take(sbuf_len)?;
    let mut out = Vec::with_capacity(n);
    for c in raw.chunks_exact(node_sz) {
        let mut q = Rd::new(c, r.be);
        let _ver = q.u16()?;
        let level = q.u8()?;
        let _flags = q.u8()?;
        let ty_off = q.u32()?;
        let name_off = q.u32()?;
        let byte_size = q.i32()?;
        let _index = q.i32()?;
        let meta = q.u32()?;
        out.push(TNode { ty: str_at(sbuf, ty_off), name: str_at(sbuf, name_off), level, byte_size, meta });
    }
    Ok(out)
}

impl SerializedFile {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        let mut h = Rd::new(buf, true);
        let _meta_size = h.u32()?;
        let _file_size = h.u32()?;
        let version = h.u32()?;
        let mut data_offset = h.u32()? as usize;
        if !(17..=22).contains(&version) {
            bail!("unsupported SerializedFile version {version}");
        }
        if h.u8()? != 0 {
            bail!("big-endian SerializedFile not supported");
        }
        h.take(3)?;
        if version >= 22 {
            let _ms = h.u32()?;
            let _fs = h.i64()?;
            data_offset = h.i64()? as usize;
            let _unk = h.i64()?;
        }
        // the header is big-endian, everything after it little-endian
        let mut r = h;
        r.be = false;
        let _unity = r.cstr()?;
        let _platform = r.i32()?;
        let type_tree = r.u8()? != 0;
        if !type_tree {
            bail!("SerializedFile without type trees");
        }
        let tc = r.i32()? as usize;
        let mut types = Vec::with_capacity(tc);
        for _ in 0..tc {
            let class_id = r.i32()?;
            let _stripped = r.u8()?;
            let script_idx = r.i16()?;
            if class_id == 114 || (class_id < 0 && script_idx >= 0) {
                r.take(16)?; // script id
            }
            r.take(16)?; // old type hash
            let tree = read_tree(&mut r, version)?;
            if version >= 21 {
                let deps = r.i32()? as usize;
                r.take(4 * deps)?;
            }
            types.push(SType { class_id, tree });
        }
        let oc = r.i32()? as usize;
        let mut objects = Vec::with_capacity(oc);
        for _ in 0..oc {
            r.align(4);
            let path_id = r.i64()?;
            let start = if version >= 22 { r.i64()? as usize } else { r.u32()? as usize };
            let size = r.u32()? as usize;
            let type_idx = r.i32()? as usize;
            if type_idx >= types.len() {
                bail!("object type index out of range");
            }
            if data_offset + start + size > buf.len() {
                bail!("object data past end of file");
            }
            objects.push(Obj { path_id, start: data_offset + start, size, type_idx });
        }
        Ok(SerializedFile { types, objects })
    }
}
