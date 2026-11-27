//! Shared by the mmvp patcher and dumper: Mabinogi Mobile package formats (`.blob`, `index`),
//! the Unity parts needed to read and replace AudioClips, and the voice site schema.
pub mod audio;
pub mod blob;
mod common_strings;
pub mod index;
mod rd;
mod serialized;
pub mod site;
mod typetree;
mod unityfs;

/// Package directories of a client install, relative to its root, as [base package, Starter
/// sub-package] per family; bit 15 of an index location selects the member.
pub static FAMILIES: [[&str; 2]; 2] = [["AssetPackages", "AssetPackages/Starter"], ["ResourcesPackage", "ResourcesPackage/Starter"]];

pub fn sha1_hex(b: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Blob number of a file name `NNNN.blob`.
pub fn blob_no(name: &str) -> Option<u16> {
    name.strip_suffix(".blob")?.parse().ok()
}
