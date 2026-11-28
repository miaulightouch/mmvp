//! The voice site the dumper writes and the patcher reads (Cloudflare Pages):
//!   latest.json     Latest
//!   manifest.json   Manifest; content only, so its SHA1 identifies the voice version
//!   packs.json      Packs: the clips concatenated per JP bundle, one download per bundle
//!   packs/<sha1>    a pack, content-addressed
//!   clips/<sha1>    raw FSB5 audio, content-addressed; only for clients up to v0.2 (they read no packs)
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const VERSION: u32 = 4;
pub const LATEST: &str = "latest.json";
pub const MANIFEST: &str = "manifest.json";
pub const PACKS: &str = "packs.json";
/// Cloudflare Pages: largest file of a deployment.
pub const MAX_FILE_SIZE: u64 = 25 << 20;

/// Container paths that are voices: `/voice/` folders, `_vo_NN` / `_voN` lines and `_vox` vocal effects.
pub fn is_voice(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("/voice/") || p.match_indices("_vo").any(|(i, _)| matches!(p.as_bytes().get(i + 3), Some(b'_' | b'x' | b'0'..=b'9')))
}

/// Below this many bytes per second of `m_Length` a clip is a silent placeholder: JP ships 288-byte,
/// 1.25 s clips (231 B/s) where it has no recording, real voices start at 1.8 kB/s.
pub const MIN_BYTES_PER_SEC: f64 = 1000.0;

/// True for a placeholder clip of `bytes` raw audio and `length` seconds; never published.
pub fn is_placeholder(bytes: usize, length: f64) -> bool {
    length > 0.0 && (bytes as f64) < MIN_BYTES_PER_SEC * length
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_paths() {
        assert!(is_voice("assets/patchableassets/sound/voice/npc/story/a.wav"));
        assert!(is_voice("…/cutscene/main/g1/c1/voice/q_main_g1_c1_0220_20_awakenglas_greymaze_vo_01.wav"));
        assert!(is_voice("…/naozone/sfx/q_opening_a1_40_portalbreakdown_naozwhite_vo1.wav"));
        assert!(is_voice("…/monster/succubus/sfx_succubus_attack_vox.wav"));
        assert!(is_voice("…/monster/x/sfx_x_vox-01.wav"));
        assert!(!is_voice("…/sfx/sfx_volcano_loop.wav"));
        assert!(!is_voice("…/bgm/bgm_sub_quest_luasong.wav"));
    }

    #[test]
    fn placeholders() {
        assert!(is_placeholder(288, 1.2466));
        assert!(!is_placeholder(3072, 1.64));
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Latest {
    pub version: u32,
    pub date: String,
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
    /// sha1 -> clip
    pub clips: BTreeMap<String, Clip>,
    /// container path -> most common sha1 (fallback when JP has no bundle of that name)
    pub default: BTreeMap<String, String>,
    /// bundle name -> container path -> sha1
    pub bundles: BTreeMap<String, BTreeMap<String, String>>,
}

/// Every clip of one manifest in exactly one pack: the pack of the first bundle (by name) that uses it.
#[derive(Serialize, Deserialize)]
pub struct Packs {
    /// the manifest these packs belong to; clients ignore packs of another version
    pub manifest_sha1: String,
    /// pack sha1 -> pack
    pub packs: BTreeMap<String, Pack>,
}

#[derive(Serialize, Deserialize)]
pub struct Pack {
    pub bundle: String,
    pub size: u64,
    /// (clip sha1, offset) in file order; a clip's length is its manifest size
    pub clips: Vec<(String, u64)>,
}

impl Manifest {
    /// JP clip for a container path: the variant of the same bundle if JP has it, else the most common one.
    pub fn lookup(&self, bundle: &str, path: &str) -> Option<&str> {
        self.bundles.get(bundle).and_then(|b| b.get(path)).or_else(|| self.default.get(path)).map(String::as_str)
    }
}
