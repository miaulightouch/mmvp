//! Generic type-tree driven object reader. Every primitive keeps its byte offset (relative to the
//! start of the buffer passed in) so fixed-size fields can be overwritten in place.
use crate::rd::Rd;
use crate::serialized::TNode;
use anyhow::{bail, Result};

const ALIGN: u32 = 0x4000;

#[derive(Clone, Debug)]
pub enum Val {
    Int { v: i64, off: usize, size: usize },
    UInt { v: u64, off: usize, size: usize },
    Float { v: f64, off: usize, size: usize },
    Bool { v: bool, off: usize },
    Str(String),
    Bytes(usize, usize),
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

impl Val {
    pub fn get(&self, k: &str) -> Option<&Val> {
        match self {
            Val::Obj(f) => f.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        if let Val::Str(s) = self { Some(s) } else { None }
    }
    pub fn int(&self) -> Option<i64> {
        match *self {
            Val::Int { v, .. } => Some(v),
            Val::UInt { v, .. } => Some(v as i64),
            _ => None,
        }
    }
    pub fn arr(&self) -> Option<&[Val]> {
        if let Val::Arr(a) = self { Some(a) } else { None }
    }
    /// (offset, size) of a primitive
    pub fn loc(&self) -> Option<(usize, usize)> {
        match *self {
            Val::Int { off, size, .. } | Val::UInt { off, size, .. } | Val::Float { off, size, .. } => Some((off, size)),
            Val::Bool { off, .. } => Some((off, 1)),
            _ => None,
        }
    }
}

fn children(tree: &[TNode], i: usize) -> Vec<usize> {
    let lvl = tree[i].level;
    let mut out = Vec::new();
    let mut j = i + 1;
    while j < tree.len() && tree[j].level > lvl {
        if tree[j].level == lvl + 1 {
            out.push(j);
        }
        j += 1;
    }
    out
}

fn read_node(tree: &[TNode], i: usize, r: &mut Rd) -> Result<Val> {
    let n = &tree[i];
    let off = r.pos;
    let v = match n.ty.as_str() {
        "SInt8" => Val::Int { v: r.u8()? as i8 as i64, off, size: 1 },
        "UInt8" | "char" => Val::UInt { v: r.u8()? as u64, off, size: 1 },
        "bool" => Val::Bool { v: r.u8()? != 0, off },
        "SInt16" | "short" => Val::Int { v: r.i16()? as i64, off, size: 2 },
        "UInt16" | "unsigned short" => Val::UInt { v: r.u16()? as u64, off, size: 2 },
        "SInt32" | "int" | "Type*" => Val::Int { v: r.i32()? as i64, off, size: 4 },
        "UInt32" | "unsigned int" => Val::UInt { v: r.u32()? as u64, off, size: 4 },
        "SInt64" | "long long" => Val::Int { v: r.i64()?, off, size: 8 },
        "UInt64" | "unsigned long long" | "FileSize" => Val::UInt { v: r.u64()?, off, size: 8 },
        "float" => Val::Float { v: f32::from_bits(r.u32()?) as f64, off, size: 4 },
        "double" => Val::Float { v: f64::from_bits(r.u64()?), off, size: 8 },
        "string" => {
            let ch = children(tree, i);
            let len = r.i32()?;
            if len < 0 {
                bail!("negative string length");
            }
            let s = String::from_utf8_lossy(r.take(len as usize)?).into_owned();
            if ch.first().map_or(false, |&a| tree[a].meta & ALIGN != 0) {
                r.align(4);
            }
            Val::Str(s)
        }
        _ => {
            let ch = children(tree, i);
            if ch.len() == 2 && tree[ch[0]].name == "size" && (n.ty == "Array" || n.ty == "TypelessData" || tree[i].name == "Array") {
                read_array(tree, i, ch[1], r)?
            } else if ch.len() == 1 && tree[ch[0]].ty == "Array" {
                // vector / map / staticvector: one Array child
                read_node(tree, ch[0], r)?
            } else {
                let mut fields = Vec::with_capacity(ch.len());
                for c in ch {
                    fields.push((tree[c].name.clone(), read_node(tree, c, r)?));
                }
                Val::Obj(fields)
            }
        }
    };
    if n.meta & ALIGN != 0 {
        r.align(4);
    }
    Ok(v)
}

fn read_array(tree: &[TNode], _arr: usize, elem: usize, r: &mut Rd) -> Result<Val> {
    let count = r.i32()?;
    if count < 0 {
        bail!("negative array size");
    }
    let count = count as usize;
    let e = &tree[elem];
    if e.byte_size == 1 && matches!(e.ty.as_str(), "UInt8" | "SInt8" | "char" | "bool") {
        let off = r.pos;
        r.take(count)?;
        return Ok(Val::Bytes(off, count)); // alignment of the array node is applied by the caller
    }
    let mut out = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        out.push(read_node(tree, elem, r)?);
    }
    Ok(Val::Arr(out))
}

/// Read a whole object. `data` is the object bytes; offsets are relative to `data`.
pub fn read_object(tree: &[TNode], data: &[u8], big_endian: bool) -> Result<Val> {
    if tree.is_empty() {
        bail!("empty type tree");
    }
    let mut r = Rd::new(data, big_endian);
    let v = read_node(tree, 0, &mut r)?;
    if r.pos != data.len() {
        bail!("type tree read {} of {} bytes", r.pos, data.len());
    }
    Ok(v)
}
