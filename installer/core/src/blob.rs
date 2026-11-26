//! `.blob` container: u32 magic, u32 hdr_end, u32[] cumulative END offsets, then the bundles.
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const BLOB_MAGIC: u32 = 0xE5A9_C3BA;

/// [(start, end)] of every slot.
pub fn read_table(path: &Path) -> Result<Vec<(u64, u64)>> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = [0u8; 8];
    f.read_exact(&mut h)?;
    let magic = u32::from_le_bytes(h[0..4].try_into().unwrap());
    let hdr_end = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
    if magic != BLOB_MAGIC {
        bail!("{}: bad blob magic {magic:#x}", path.display());
    }
    let n = ((hdr_end - 8) / 4) as usize;
    let mut raw = vec![0u8; n * 4];
    f.read_exact(&mut raw)?;
    let mut out = Vec::with_capacity(n);
    let mut prev = hdr_end;
    for c in raw.chunks_exact(4) {
        let e = u32::from_le_bytes(c.try_into().unwrap()) as u64;
        out.push((prev, e));
        prev = e;
    }
    Ok(out)
}

pub fn read_slot(path: &Path, slot: usize) -> Result<Vec<u8>> {
    let t = read_table(path)?;
    let Some(&(s, e)) = t.get(slot) else { bail!("{}: no slot {slot}", path.display()) };
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(s))?;
    let mut buf = vec![0u8; (e - s) as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Append-only blob writer: bundles are appended as they arrive (slot = arrival order),
/// the offset table is written by `finish`. Writes to `<path>.mmvoice.tmp`.
pub struct BlobWriter {
    pub path: PathBuf,
    pub tmp: PathBuf,
    count: usize,
    hdr_end: u64,
    ends: Vec<u32>,
    pos: u64,
    f: Option<File>,
}

impl BlobWriter {
    pub fn create(path: PathBuf, count: usize) -> Result<Self> {
        let tmp = PathBuf::from(format!("{}.mmvoice.tmp", path.display()));
        let hdr_end = 8 + 4 * count as u64;
        let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.seek(SeekFrom::Start(hdr_end))?;
        Ok(BlobWriter { path, tmp, count, hdr_end, ends: Vec::with_capacity(count), pos: hdr_end, f: Some(f) })
    }

    /// -> slot index of the appended bundle
    pub fn append(&mut self, data: &[u8]) -> Result<u16> {
        self.f.as_mut().unwrap().write_all(data)?;
        self.pos += data.len() as u64;
        if self.pos >= 1 << 32 {
            bail!("{}: would exceed 4 GiB", self.path.display());
        }
        self.ends.push(self.pos as u32);
        Ok((self.ends.len() - 1) as u16)
    }

    pub fn finish(&mut self) -> Result<()> {
        if self.ends.len() != self.count {
            bail!("{}: {} of {} bundles written", self.path.display(), self.ends.len(), self.count);
        }
        let mut f = self.f.take().unwrap();
        f.seek(SeekFrom::Start(0))?;
        let mut hdr = Vec::with_capacity(self.hdr_end as usize);
        hdr.extend_from_slice(&BLOB_MAGIC.to_le_bytes());
        hdr.extend_from_slice(&(self.hdr_end as u32).to_le_bytes());
        for e in &self.ends {
            hdr.extend_from_slice(&e.to_le_bytes());
        }
        f.write_all(&hdr)?;
        f.sync_all()?;
        Ok(())
    }

    pub fn abort(&mut self) {
        self.f.take();
        let _ = std::fs::remove_file(&self.tmp);
    }
}
