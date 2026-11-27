//! Type-tree driven object reader (little-endian). Primitives keep their byte offset relative to the
//! object start so fixed-size fields can be overwritten in place.
use crate::rd::Rd;
use crate::serialized::TNode;
use anyhow::{bail, Result};

const ALIGN: u32 = 0x4000;

#[derive(Clone, Debug)]
pub enum Val {
    Int { v: i64, off: usize, size: usize },
    Bool { v: bool, off: usize },
    Float { v: f64, off: usize, size: usize },
    Str(String),
    /// byte arrays are skipped
    Bytes,
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
        if let Val::Int { v, .. } = self { Some(*v) } else { None }
    }
    pub fn arr(&self) -> Option<&[Val]> {
        if let Val::Arr(a) = self { Some(a) } else { None }
    }
    /// (offset, size) of a primitive
    pub fn loc(&self) -> Option<(usize, usize)> {
        match *self {
            Val::Int { off, size, .. } | Val::Float { off, size, .. } => Some((off, size)),
            Val::Bool { off, .. } => Some((off, 1)),
            _ => None,
        }
    }
    /// JSON value of a primitive (bools as booleans, floats widened from f32 like Python does)
    pub fn json(&self) -> Option<serde_json::Value> {
        match *self {
            Val::Int { v, .. } => Some(v.into()),
            Val::Bool { v, .. } => Some(v.into()),
            Val::Float { v, .. } => serde_json::Number::from_f64(v).map(Into::into),
            _ => None,
        }
    }
}

fn children(tree: &[TNode], i: usize) -> Vec<usize> {
    let lvl = tree[i].level;
    tree[i + 1..].iter().take_while(|n| n.level > lvl).enumerate().filter(|(_, n)| n.level == lvl + 1).map(|(k, _)| i + 1 + k).collect()
}

fn read_node(tree: &[TNode], i: usize, r: &mut Rd) -> Result<Val> {
    let n = &tree[i];
    let off = r.pos;
    let int = |v: i64, size| Val::Int { v, off, size };
    let v = match n.ty.as_str() {
        "SInt8" => int(r.u8()? as i8 as i64, 1),
        "UInt8" | "char" => int(r.u8()? as i64, 1),
        "bool" => Val::Bool { v: r.u8()? != 0, off },
        "SInt16" | "short" => int(r.i16()? as i64, 2),
        "UInt16" | "unsigned short" => int(r.u16()? as i64, 2),
        "SInt32" | "int" | "Type*" => int(r.i32()? as i64, 4),
        "UInt32" | "unsigned int" => int(r.u32()? as i64, 4),
        "SInt64" | "long long" => int(r.i64()?, 8),
        "UInt64" | "unsigned long long" | "FileSize" => int(r.u64()? as i64, 8),
        "float" => Val::Float { v: f32::from_bits(r.u32()?) as f64, off, size: 4 },
        "double" => Val::Float { v: f64::from_bits(r.u64()?), off, size: 8 },
        _ => {
            let ch = children(tree, i);
            if n.ty == "string" {
                let len = usize::try_from(r.i32()?)?;
                let s = String::from_utf8_lossy(r.take(len)?).into_owned();
                if ch.first().is_some_and(|&a| tree[a].meta & ALIGN != 0) {
                    r.align(4);
                }
                Val::Str(s)
            } else if ch.len() == 2 && tree[ch[0]].name == "size" && (n.ty == "Array" || n.ty == "TypelessData" || n.name == "Array") {
                let count = usize::try_from(r.i32()?)?;
                let e = &tree[ch[1]];
                if e.byte_size == 1 && matches!(e.ty.as_str(), "UInt8" | "SInt8" | "char" | "bool") {
                    r.take(count)?;
                    Val::Bytes
                } else {
                    Val::Arr((0..count).map(|_| read_node(tree, ch[1], r)).collect::<Result<_>>()?)
                }
            } else if ch.len() == 1 && tree[ch[0]].ty == "Array" {
                read_node(tree, ch[0], r)? // vector / map: one Array child
            } else {
                Val::Obj(ch.iter().map(|&c| Ok((tree[c].name.clone(), read_node(tree, c, r)?))).collect::<Result<_>>()?)
            }
        }
    };
    if n.meta & ALIGN != 0 {
        r.align(4);
    }
    Ok(v)
}

/// Read a whole object; `data` is exactly the object's bytes.
pub fn read_object(tree: &[TNode], data: &[u8]) -> Result<Val> {
    if tree.is_empty() {
        bail!("empty type tree");
    }
    let mut r = Rd::new(data, false);
    let v = read_node(tree, 0, &mut r)?;
    if r.pos != data.len() {
        bail!("type tree read {} of {} bytes", r.pos, data.len());
    }
    Ok(v)
}
