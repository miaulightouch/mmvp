//! `.blob` container: u32 magic, u32 header end, u32[] cumulative end offset of each slot, bundles.
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const MAGIC: u32 = 0xE5A9_C3BA;

/// (start, end) of every slot; `r` is at the start of a blob (a file, or its bytes).
pub fn table(r: &mut impl Read) -> Result<Vec<(u64, u64)>> {
    let mut h = [0u8; 8];
    r.read_exact(&mut h)?;
    let (magic, hdr_end) = (u32::from_le_bytes(h[..4].try_into()?), u32::from_le_bytes(h[4..].try_into()?) as u64);
    if magic != MAGIC || hdr_end < 8 {
        bail!("not a blob");
    }
    let mut raw = vec![0u8; (hdr_end - 8) as usize];
    r.read_exact(&mut raw)?;
    let mut prev = hdr_end;
    Ok(raw
        .chunks_exact(4)
        .map(|c| {
            let end = u32::from_le_bytes(c.try_into().unwrap()) as u64;
            (std::mem::replace(&mut prev, end), end)
        })
        .collect())
}

/// Append-only blob writer: bundles go in as they arrive (slot = arrival order), `finish` writes the
/// offset table. Dropping an unfinished writer deletes the file.
pub struct Writer {
    path: PathBuf,
    f: Option<File>, // None once finished
    count: usize,
    ends: Vec<u32>,
    pos: u64,
}

impl Writer {
    /// A blob of exactly `count` bundles at `path` (replaced if it exists).
    pub fn create(path: PathBuf, count: usize) -> Result<Self> {
        if count > u16::MAX as usize + 1 {
            bail!("{}: {count} bundles do not fit a blob", path.display());
        }
        let mut f = File::create(&path).with_context(|| format!("create {}", path.display()))?;
        let hdr_end = 8 + 4 * count as u64;
        f.seek(SeekFrom::Start(hdr_end))?;
        Ok(Writer { path, f: Some(f), count, ends: Vec::with_capacity(count), pos: hdr_end })
    }

    /// -> slot of the appended bundle
    pub fn append(&mut self, data: &[u8]) -> Result<u16> {
        self.f.as_mut().unwrap().write_all(data)?;
        self.pos += data.len() as u64;
        if self.pos > u32::MAX as u64 {
            bail!("{}: over 4 GiB", self.path.display());
        }
        self.ends.push(self.pos as u32);
        Ok((self.ends.len() - 1) as u16)
    }

    pub fn finish(mut self) -> Result<()> {
        if self.ends.len() != self.count {
            bail!("{}: {} of {} bundles written", self.path.display(), self.ends.len(), self.count);
        }
        let mut hdr = Vec::with_capacity(8 + 4 * self.count);
        hdr.extend(MAGIC.to_le_bytes());
        hdr.extend((8 + 4 * self.count as u32).to_le_bytes());
        self.ends.iter().for_each(|e| hdr.extend(e.to_le_bytes()));
        let f = self.f.as_mut().unwrap();
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&hdr)?;
        f.sync_all()?;
        self.f = None;
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            drop(f); // close before deleting
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("mmvp-blob-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("9876.blob");
        let mut w = Writer::create(path.clone(), 2).unwrap();
        assert_eq!(w.append(b"abc").unwrap(), 0);
        assert_eq!(w.append(b"defgh").unwrap(), 1);
        w.finish().unwrap();
        assert_eq!(table(&mut File::open(&path).unwrap()).unwrap(), vec![(16, 19), (19, 24)]);
        assert_eq!(table(&mut &std::fs::read(&path).unwrap()[..]).unwrap().len(), 2);
        let unfinished = dir.join("1.blob");
        drop(Writer::create(unfinished.clone(), 1).unwrap());
        assert!(!unfinished.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
