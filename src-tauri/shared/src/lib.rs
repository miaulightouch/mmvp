//! Shared by the mmvp patcher and dumper: Mabinogi Mobile package formats (`.blob`, `index`),
//! the Unity parts needed to read and replace AudioClips, the voice site schema, and HTTP.
pub mod audio;
pub mod blob;
mod common_strings;
pub mod index;
pub mod net;
mod rd;
mod serialized;
pub mod site;
mod typetree;
mod unityfs;

/// Package directories of a client install, relative to its root.
pub const PACKAGES: [&str; 4] = ["AssetPackages", "AssetPackages/Starter", "ResourcesPackage", "ResourcesPackage/Starter"];

pub fn sha1_hex(b: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Blob number of a file name `NNNN.blob`.
pub fn blob_no(name: &str) -> Option<u16> {
    let stem = name.strip_suffix(".blob")?;
    (!stem.is_empty() && stem.bytes().all(|c| c.is_ascii_digit())).then(|| stem.parse().ok()).flatten()
}
