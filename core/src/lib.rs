//! mmvp core: patch a TW Mabinogi Mobile install with JP voices downloaded from the voice site
//! (tools/mmvp_dump.py), by redirecting the package index to rebuilt bundles.
pub mod audio;
pub mod blob;
mod common_strings;
pub mod index;
pub mod install;
pub mod net;
mod rd;
mod serialized;
mod typetree;
mod unityfs;
pub mod voices;

pub use install::{Install, State, Status, Step};
pub use voices::{Source, Voices};

pub fn sha1_hex(b: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}
