//! The voice site the dumper writes and the patcher reads (Cloudflare Pages):
//!   latest.json     Latest
//!   manifest.json   Manifest; content only, so its SHA1 identifies the voice version
//!   clips/<sha1>    raw FSB5 audio, content-addressed
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const VERSION: u32 = 3;
/// Container paths that are voices.
pub fn is_voice(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("/voice/") || p.contains("_vo_")
}
pub const VOICE_FILTER: &str = "(/voice/|_vo_)";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Latest {
    pub version: u32,
    pub date: String,
    pub manifest: String,
    pub manifest_sha1: String,
    pub clips: usize,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Clip {
    pub size: u64,
    /// AudioClip format fields (m_Length, m_Channels, …) in field order
    pub fields: Map<String, Value>,
}

/// Written as compact JSON. Field order is part of the format: the manifest SHA1 is the voice version id.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub filter: String,
    /// sha1 -> clip
    pub clips: BTreeMap<String, Clip>,
    /// container path -> most common sha1 (fallback when JP has no bundle of that name)
    pub default: BTreeMap<String, String>,
    /// bundle name -> container path -> sha1
    pub bundles: BTreeMap<String, BTreeMap<String, String>>,
}

impl Manifest {
    /// JP clip for a container path: the variant of the same bundle if JP has it, else the most common one.
    pub fn lookup(&self, bundle: &str, path: &str) -> Option<&str> {
        self.bundles.get(bundle).and_then(|b| b.get(path)).or_else(|| self.default.get(path)).map(String::as_str)
    }
}
