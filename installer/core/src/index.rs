//! Package `index` ([確認] layout, little-endian):
//!   u32 magic 0xE6A9C3BA, u32 N
//!   (u16 blob, u16 slot)[N]                         asset entry -> bundle location
//!   u32 M, M x { (u16 blob, u16 slot) key, u32 C, (u16 blob, u16 slot)[C] }
//!                                                   bundle location -> locations of its dependencies
//!   u32 N, 16-byte value[N], u32[] ...              per-entry data without locations, kept as-is
//! The blob field's bit 15 selects the package: 0 = the family's base package (AssetPackages,
//! ResourcesPackage), 1 = its Starter sub-package ([確認] every entry of a */Starter main table has it,
//! and dependency lists mix both). A redirected bundle must be rewritten in both tables: the game
//! looks up a bundle's dependencies by the location it loads it from.
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

pub const INDEX_MAGIC: u32 = 0xE6A9_C3BA;
/// Bit 15 of the blob field: the bundle lives in the Starter sub-package.
pub const FLAG: u16 = 0x8000;
pub const MASK: u16 = 0x7FFF;
/// Blob numbers >= this are ours (originals stop around 800).
pub const REDIRECT_BASE: u16 = 5000;

pub type Key = (u16, u16);
/// Redirects per package of a family: [base package, Starter sub-package].
pub type FamilyRedirect<'a> = [&'a HashMap<Key, Key>; 2];

fn rd_u32(buf: &[u8], p: usize) -> Result<u32> {
    match buf.get(p..p + 4) {
        Some(b) => Ok(u32::from_le_bytes(b.try_into().unwrap())),
        None => bail!("index truncated"),
    }
}

/// Byte offsets of every (blob, slot) location in the index: main entries and dependency table.
/// -> (main entry offsets, dependency table offsets)
fn locations(buf: &[u8]) -> Result<(Vec<usize>, Vec<usize>)> {
    if rd_u32(buf, 0)? != INDEX_MAGIC {
        bail!("bad index magic");
    }
    let n = rd_u32(buf, 4)? as usize;
    let main: Vec<usize> = (0..n).map(|k| 8 + 4 * k).collect();
    let mut p = 8 + 4 * n;
    let m = rd_u32(buf, p)? as usize;
    p += 4;
    let mut deps = Vec::new();
    for _ in 0..m {
        deps.push(p);
        let c = rd_u32(buf, p + 4)? as usize;
        p += 8;
        for _ in 0..c {
            deps.push(p);
            p += 4;
        }
        if p > buf.len() {
            bail!("index dependency table truncated");
        }
    }
    // sanity: the next section starts with the entry count again
    if rd_u32(buf, p)? as usize != n {
        bail!("unexpected index layout after the dependency table");
    }
    Ok((main, deps))
}

fn pair(buf: &[u8], o: usize) -> (u16, u16) {
    (u16::from_le_bytes([buf[o], buf[o + 1]]), u16::from_le_bytes([buf[o + 2], buf[o + 3]]))
}

/// (blob, slot) locations of the main entry table, flag masked off.
pub fn slots(buf: &[u8]) -> Result<HashSet<Key>> {
    let (main, _) = locations(buf)?;
    Ok(main.iter().map(|&o| { let (b, s) = pair(buf, o); (b & MASK, s) }).collect())
}

/// True if the index was written by us (some location points at a blob >= REDIRECT_BASE).
pub fn is_patched(buf: &[u8]) -> Result<bool> {
    Ok(!redirect_blobs(buf)?.is_empty())
}

/// Redirect blob numbers in use anywhere in a (patched) index.
pub fn redirect_blobs(buf: &[u8]) -> Result<HashSet<u16>> {
    let (main, deps) = locations(buf)?;
    Ok(main.iter().chain(&deps).map(|&o| pair(buf, o).0 & MASK).filter(|&b| b >= REDIRECT_BASE).collect())
}

/// Number of main entries pointing at our blobs.
pub fn redirected_entries(buf: &[u8]) -> Result<usize> {
    let (main, _) = locations(buf)?;
    Ok(main.iter().filter(|&&o| pair(buf, o).0 & MASK >= REDIRECT_BASE).count())
}

/// Rewrite every location (main entries and dependency table) found in `red[flag]`; the flag is kept.
/// -> (new index bytes, main entries changed, dependency locations changed)
pub fn patch(buf: &[u8], red: FamilyRedirect) -> Result<(Vec<u8>, usize, usize)> {
    let (main, deps) = locations(buf)?;
    let mut out = buf.to_vec();
    let mut apply = |o: usize| -> bool {
        let (b, s) = pair(buf, o);
        let flag = b & FLAG;
        match red[(flag >> 15) as usize].get(&(b & MASK, s)) {
            Some(&(nb, ns)) => {
                out[o..o + 2].copy_from_slice(&(flag | nb).to_le_bytes());
                out[o + 2..o + 4].copy_from_slice(&ns.to_le_bytes());
                true
            }
            None => false,
        }
    };
    let changed_main = main.iter().filter(|&&o| apply(o)).count();
    let changed_deps = deps.iter().filter(|&&o| apply(o)).count();
    Ok((out, changed_main, changed_deps))
}
