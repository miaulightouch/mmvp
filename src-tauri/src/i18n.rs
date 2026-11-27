//! Language of the messages the backend produces (errors, native dialog and window titles): English
//! or Traditional Chinese. The front end picks the language (src/i18n.js) and reports it at start.
use std::sync::atomic::{AtomicBool, Ordering};

static ZH: AtomicBool = AtomicBool::new(false);

pub fn set_zh(zh: bool) {
    ZH.store(zh, Ordering::Relaxed);
}

pub fn zh() -> bool {
    ZH.load(Ordering::Relaxed)
}

/// `tr!("English {x}", "中文 {x}", args…)`: the message in the GUI language, formatted like `format!`.
macro_rules! tr {
    ($en:literal, $zh:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::zh() { format!($zh $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}
