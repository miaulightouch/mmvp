//! `.blob` container: u32 magic, u32 header end, u32[] cumulative end offset of each slot, bundles.
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: u32 = 0xE5A9_C3BA;

/// (start, end) of every slot.
pub fn read_table(path: &Path) -> Result<Vec<(u64, u64)>> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = [0u8; 8];
    f.read_exact(&mut h)?;
    let (magic, hdr_end) = (u32::from_le_bytes(h[..4].try_into()?), u32::from_le_bytes(h[4..].try_into()?) as u64);
    if magic != MAGIC || hdr_end < 8 {
        bail!("{}: not a blob", path.display());
    }
    let mut raw = vec![0u8; (hdr_end - 8) as usize];
    f.read_exact(&mut raw)?;
    let mut prev = hdr_end;
    Ok(raw
        .chunks_exact(4)
        .map(|c| {
            let end = u32::from_le_bytes(c.try_into().unwrap()) as u64;
            (std::mem::replace(&mut prev, end), end)
        })
        .collect())
}

/// Append-only blob writer: bundles go in as they arrive (slot = arrival order) into `<path>.mmvp.tmp`;
/// `finish` writes the offset table and renames nothing. Dropping an unfinished writer deletes the tmp file.
pub struct Writer {
    pub path: PathBuf,
    pub tmp: PathBuf,
    f: Option<File>,
    count: usize,
    ends: Vec<u32>,
    pos: u64,
    done: bool,
}

impl Writer {
    pub fn create(path: PathBuf, count: usize) -> Result<Self> {
        let tmp = path.with_extension("blob.mmvp.tmp");
        let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        let hdr_end = 8 + 4 * count as u64;
        f.seek(SeekFrom::Start(hdr_end))?;
        Ok(Writer { path, tmp, f: Some(f), count, ends: Vec::with_capacity(count), pos: hdr_end, done: false })
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

    pub fn finish(&mut self) -> Result<()> {
        if self.ends.len() != self.count {
            bail!("{}: {} of {} bundles written", self.path.display(), self.ends.len(), self.count);
        }
        let mut f = self.f.take().unwrap();
        let mut hdr = Vec::with_capacity(8 + 4 * self.count);
        hdr.extend(MAGIC.to_le_bytes());
        hdr.extend((8 + 4 * self.count as u32).to_le_bytes());
        self.ends.iter().for_each(|e| hdr.extend(e.to_le_bytes()));
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&hdr)?;
        f.sync_all()?;
        self.done = true;
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if !self.done {
            self.f.take();
            let _ = std::fs::remove_file(&self.tmp);
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
        std::fs::rename(&w.tmp, &path).unwrap();
        assert_eq!(read_table(&path).unwrap(), vec![(16, 19), (19, 24)]);
        let unfinished = Writer::create(dir.join("1.blob"), 1).unwrap();
        let tmp = unfinished.tmp.clone();
        drop(unfinished);
        assert!(!tmp.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
