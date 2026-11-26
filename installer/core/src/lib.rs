//! Mabinogi Mobile JP voice patcher core: reads a JP voice pack (.mmvpack v2, made by
//! tools/mmvoice.py dump) and patches a TW install by index redirect. See README.md.
pub mod audio;
pub mod blob;
mod common_strings;
pub mod index;
pub mod pack;
pub mod patcher;
mod rd;
pub mod serialized;
pub mod typetree;
pub mod unityfs;
