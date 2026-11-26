//! `.mmvpack` (version 2): stored ZIP with manifest.json + clips/<sha1>. Written by tools/mmvoice.py dump.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub created: String,
    /// sha1 -> AudioClip format fields
    pub clips: HashMap<String, Map<String, Value>>,
    /// bundle name -> container path -> sha1
    pub bundles: HashMap<String, HashMap<String, String>>,
    /// container path -> most common sha1
    pub default: HashMap<String, String>,
}

impl Manifest {
    /// JP variant for this exact bundle if JP has it, else the most common variant of the path.
    pub fn lookup(&self, bundle: &str, path: &str) -> Option<&str> {
        self.bundles.get(bundle).and_then(|m| m.get(path)).or_else(|| self.default.get(path)).map(|s| s.as_str())
    }
}

pub struct Pack {
    pub path: PathBuf,
    pub manifest: Manifest,
    /// sha1 of manifest.json: identifies the pack content
    pub id: String,
}

impl Pack {
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("open pack {}", path.display()))?;
        let mut z = zip::ZipArchive::new(f).context("pack is not a zip")?;
        let mut raw = Vec::new();
        z.by_name("manifest.json").context("pack has no manifest.json")?.read_to_end(&mut raw)?;
        let manifest: Manifest = serde_json::from_slice(&raw).context("bad manifest.json")?;
        if manifest.version != 2 {
            bail!("pack version {} not supported (need 2)", manifest.version);
        }
        Ok(Pack { path: path.to_path_buf(), manifest, id: crate::audio::sha1_hex(&raw) })
    }

    /// One reader per thread (ZipArchive needs &mut).
    pub fn reader(&self) -> Result<PackReader> {
        Ok(PackReader { z: zip::ZipArchive::new(File::open(&self.path)?)? })
    }
}

pub struct PackReader {
    z: zip::ZipArchive<File>,
}

impl PackReader {
    pub fn clip(&mut self, sha1: &str) -> Result<Vec<u8>> {
        let mut e = self.z.by_name(&format!("clips/{sha1}")).with_context(|| format!("pack has no clip {sha1}"))?;
        let mut v = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut v)?;
        if crate::audio::sha1_hex(&v) != sha1 {
            bail!("pack clip {sha1} is corrupt");
        }
        Ok(v)
    }
}
