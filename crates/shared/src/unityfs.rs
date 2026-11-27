//! UnityFS asset bundle (format 6-8): parse, lazy block decompression, and an LZ4 writer that
//! reproduces the layout UnityPy's `save(packer="lz4")` produced for the version verified in game:
//! flags 0xC2 (LZ4 | directory combined | block info at the end), 128 KiB chunks, a chunk that does
//! not shrink is stored raw (flag 0).
use crate::rd::{align_up, Rd};
use anyhow::{bail, Context, Result};

const CHUNK: usize = 0x20000;
const F_COMP_MASK: u32 = 0x3F;
const F_COMBINED: u32 = 0x40;
const F_INFO_AT_END: u32 = 0x80;
const F_PAD_AT_START: u32 = 0x200;
const F_ENCRYPTED: u32 = 0x400; // ArchiveFlags.UsesAssetBundleEncryption (2020.x+ numbering)

#[derive(Clone, Debug)]
pub struct Node {
    pub offset: u64,
    pub size: u64,
    pub flags: u32,
    pub name: String,
}

struct Block {
    usize: u32,
    csize: u32,
    flags: u16,
    src: usize, // offset of compressed data in the bundle bytes
}

pub struct Bundle<'a> {
    raw: &'a [u8],
    pub version: u32,
    pub player_ver: String,
    pub engine_ver: String,
    blocks: Vec<Block>,
    pub nodes: Vec<Node>,
    /// decompressed data stream; filled block by block
    data: Vec<u8>,
    next_block: usize,
}

fn decompress(src: &[u8], usize: usize, comp: u32) -> Result<Vec<u8>> {
    match comp {
        0 => Ok(src.to_vec()),
        2 | 3 => lz4_flex::block::decompress(src, usize).context("lz4 decompress"),
        c => bail!("unsupported compression {c}"),
    }
}

impl<'a> Bundle<'a> {
    pub fn parse(raw: &'a [u8]) -> Result<Self> {
        let mut r = Rd::new(raw, true);
        if r.cstr()? != "UnityFS" {
            bail!("not UnityFS");
        }
        let version = r.u32()?;
        let player_ver = r.cstr()?;
        let engine_ver = r.cstr()?;
        let _size = r.i64()?;
        let ci_size = r.u32()? as usize;
        let ui_size = r.u32()? as usize;
        let flags = r.u32()?;
        if flags & F_ENCRYPTED != 0 {
            bail!("encrypted bundle");
        }
        if version >= 7 {
            r.align(16);
        }
        let info_c = if flags & F_INFO_AT_END != 0 {
            &raw[raw.len() - ci_size..]
        } else {
            r.take(ci_size)?
        };
        if flags & F_PAD_AT_START != 0 && flags & F_INFO_AT_END == 0 {
            r.align(16);
        }
        let info = decompress(info_c, ui_size, flags & F_COMP_MASK)?;
        let mut b = Rd::new(&info, true);
        b.take(16)?; // uncompressed data hash
        let nb = b.i32()? as usize;
        let mut blocks = Vec::with_capacity(nb);
        let mut src = r.pos;
        for _ in 0..nb {
            let (u, c, f) = (b.u32()?, b.u32()?, b.u16()?);
            blocks.push(Block { usize: u, csize: c, flags: f, src });
            src += c as usize;
        }
        if src > raw.len() {
            bail!("block data past end of bundle");
        }
        let nn = b.i32()? as usize;
        let mut nodes = Vec::with_capacity(nn);
        for _ in 0..nn {
            nodes.push(Node { offset: b.u64()?, size: b.u64()?, flags: b.u32()?, name: b.cstr()? });
        }
        let total: usize = blocks.iter().map(|b| b.usize as usize).sum();
        Ok(Bundle { raw, version, player_ver, engine_ver, blocks, nodes, data: Vec::with_capacity(total), next_block: 0 })
    }

    /// Make sure the decompressed stream covers [0, end).
    pub fn ensure(&mut self, end: usize) -> Result<()> {
        while self.data.len() < end && self.next_block < self.blocks.len() {
            let blk = &self.blocks[self.next_block];
            let src = &self.raw[blk.src..blk.src + blk.csize as usize];
            let d = decompress(src, blk.usize as usize, (blk.flags as u32) & F_COMP_MASK)?;
            self.data.extend_from_slice(&d);
            self.next_block += 1;
        }
        if self.data.len() < end {
            bail!("bundle data shorter than node table says");
        }
        Ok(())
    }

    pub fn node_data(&mut self, i: usize) -> Result<&[u8]> {
        let (o, s) = (self.nodes[i].offset as usize, self.nodes[i].size as usize);
        self.ensure(o + s)?;
        Ok(&self.data[o..o + s])
    }

    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
    }
}

/// Write a UnityFS bundle with the given nodes (flags, name, data), LZ4 chunks, block info at the end.
pub fn write(like: &Bundle, nodes: &[(u32, &str, &[u8])]) -> Vec<u8> {
    write_raw(like.version, &like.player_ver, &like.engine_ver, nodes)
}

fn write_raw(version: u32, player_ver: &str, engine_ver: &str, nodes: &[(u32, &str, &[u8])]) -> Vec<u8> {
    let total: usize = nodes.iter().map(|n| n.2.len()).sum();
    let mut file_data = Vec::with_capacity(total);
    for n in nodes {
        file_data.extend_from_slice(n.2);
    }
    let mut packed = Vec::with_capacity(total);
    let mut blocks: Vec<(u32, u32, u16)> = Vec::new();
    for chunk in file_data.chunks(CHUNK) {
        let c = lz4_flex::block::compress(chunk);
        if c.len() >= chunk.len() {
            packed.extend_from_slice(chunk);
            blocks.push((chunk.len() as u32, chunk.len() as u32, 0));
        } else {
            packed.extend_from_slice(&c);
            blocks.push((chunk.len() as u32, c.len() as u32, 2));
        }
    }
    let mut info = vec![0u8; 16];
    info.extend_from_slice(&(blocks.len() as i32).to_be_bytes());
    for (u, c, f) in &blocks {
        info.extend_from_slice(&u.to_be_bytes());
        info.extend_from_slice(&c.to_be_bytes());
        info.extend_from_slice(&f.to_be_bytes());
    }
    info.extend_from_slice(&(nodes.len() as i32).to_be_bytes());
    let mut off = 0u64;
    for (flags, name, data) in nodes {
        info.extend_from_slice(&off.to_be_bytes());
        info.extend_from_slice(&(data.len() as u64).to_be_bytes());
        info.extend_from_slice(&flags.to_be_bytes());
        info.extend_from_slice(name.as_bytes());
        info.push(0);
        off += data.len() as u64;
    }
    let info_c = lz4_flex::block::compress(&info);

    let mut out = Vec::with_capacity(packed.len() + info_c.len() + 128);
    out.extend_from_slice(b"UnityFS\0");
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(player_ver.as_bytes());
    out.push(0);
    out.extend_from_slice(engine_ver.as_bytes());
    out.push(0);
    let size_pos = out.len();
    out.extend_from_slice(&0i64.to_be_bytes());
    out.extend_from_slice(&(info_c.len() as u32).to_be_bytes());
    out.extend_from_slice(&(info.len() as u32).to_be_bytes());
    out.extend_from_slice(&(2 | F_COMBINED | F_INFO_AT_END).to_be_bytes());
    if version >= 7 {
        out.resize(align_up(out.len(), 16), 0);
    }
    out.extend_from_slice(&packed);
    out.extend_from_slice(&info_c);
    let size = out.len() as i64;
    out[size_pos..size_pos + 8].copy_from_slice(&size.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_parse() {
        let header = {
            let mut h = Vec::new();
            h.extend(b"UnityFS\0");
            h.extend(8u32.to_be_bytes());
            h.extend(b"5.x.x\x002021.3.51f1\0");
            h
        };
        // a minimal bundle to carry the version strings
        let empty = write_raw(8, "5.x.x", "2021.3.51f1", &[]);
        assert!(empty.starts_with(&header));
        let like = Bundle::parse(&empty).unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let out = write(&like, &[(4, "CAB-x", b"serialized".as_slice()), (0, "CAB-x.resource", &big)]);
        let mut b = Bundle::parse(&out).unwrap();
        assert_eq!(b.nodes.len(), 2);
        assert_eq!(b.node_data(0).unwrap(), b"serialized");
        assert_eq!(b.node_data(1).unwrap(), big.as_slice());
        assert_eq!(b.nodes[1].flags, 0);
    }
}
