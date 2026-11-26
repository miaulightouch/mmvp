//! Mabinogi Mobile JP voice patcher core: downloads JP voices from the static site made by
//! tools/dump_site.py and patches a TW install by index redirect. See README.md.
pub mod audio;
pub mod blob;
mod common_strings;
pub mod index;
pub mod patcher;
mod rd;
pub mod serialized;
pub mod typetree;
pub mod unityfs;
pub mod voices;
