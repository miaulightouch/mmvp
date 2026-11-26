//! Package `index`: u32 magic, u32 N, (u16 blob, u16 slot)[N], then a compressed path table we keep as-is.
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

pub const INDEX_MAGIC: u32 = 0xE6A9_C3BA;
/// Set on every entry of the */Starter indexes (blob = 0x8000 | NNNN). Meaning unknown; kept as-is.
pub const FLAG: u16 = 0x8000;
pub const MASK: u16 = 0x7FFF;
/// Blob numbers >= this are ours (originals stop around 800).
pub const REDIRECT_BASE: u16 = 5000;

pub type Key = (u16, u16);

fn entries(buf: &[u8]) -> Result<(usize, &[u8])> {
    if buf.len() < 8 {
        bail!("index too short");
    }
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    if magic != INDEX_MAGIC {
        bail!("bad index magic {magic:#x}");
    }
    let n = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    if buf.len() < 8 + 4 * n {
        bail!("index truncated");
    }
    Ok((n, &buf[8..8 + 4 * n]))
}

fn pair(c: &[u8]) -> (u16, u16) {
    (u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]))
}

/// (blob, slot) pairs referenced by the index, flag masked off.
pub fn slots(buf: &[u8]) -> Result<HashSet<Key>> {
    let (_, e) = entries(buf)?;
    Ok(e.chunks_exact(4).map(|c| { let (b, s) = pair(c); (b & MASK, s) }).collect())
}

/// True if the index was written by us (some entry points at a blob >= REDIRECT_BASE).
pub fn is_patched(buf: &[u8]) -> Result<bool> {
    let (_, e) = entries(buf)?;
    Ok(e.chunks_exact(4).any(|c| pair(c).0 & MASK >= REDIRECT_BASE))
}

/// Redirect blob numbers in use by a (patched) index.
pub fn redirect_blobs(buf: &[u8]) -> Result<HashSet<u16>> {
    let (_, e) = entries(buf)?;
    Ok(e.chunks_exact(4).map(|c| pair(c).0 & MASK).filter(|&b| b >= REDIRECT_BASE).collect())
}

/// -> (new index bytes, entries changed). Matching ignores the flag bit; the flag is kept.
pub fn patch(buf: &[u8], redirect: &HashMap<Key, Key>) -> Result<(Vec<u8>, usize)> {
    entries(buf)?;
    let mut out = buf.to_vec();
    let n = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    let mut changed = 0;
    for k in 0..n {
        let o = 8 + 4 * k;
        let (b, s) = pair(&out[o..o + 4]);
        if let Some(&(nb, ns)) = redirect.get(&(b & MASK, s)) {
            out[o..o + 2].copy_from_slice(&((b & FLAG) | nb).to_le_bytes());
            out[o + 2..o + 4].copy_from_slice(&ns.to_le_bytes());
            changed += 1;
        }
    }
    Ok((out, changed))
}
