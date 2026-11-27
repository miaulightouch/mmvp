//! Patching: download JP voices from the voice site and apply them to a TW install by redirecting the
//! package index to rebuilt bundles. Independent of the GUI (used by the end-to-end test as well).
pub mod install;
pub mod voices;

pub use install::{Install, State, Status, Step};
pub use voices::{Source, Voices};
