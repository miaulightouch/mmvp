//! End-to-end check against a real (copied!) TW install; skipped unless both are set:
//!   MMVP_TEST_ROOT    a copy of …\MabinogiMobile_TW\appdata (it gets patched and restored)
//!   MMVP_TEST_SOURCE  voice site URL or a local mmvp_dump.py output directory
//! cargo test -p mmvp --release -- --nocapture
use mmvp_lib::patcher::{Install, Source, State, Voices};
use std::path::PathBuf;

#[test]
fn apply_reapply_restore() {
    let (Ok(root), Ok(src)) = (std::env::var("MMVP_TEST_ROOT"), std::env::var("MMVP_TEST_SOURCE")) else {
        eprintln!("skipped: MMVP_TEST_ROOT / MMVP_TEST_SOURCE not set");
        return;
    };
    let root = PathBuf::from(root);
    let install = Install::open(&root).unwrap();
    let cache = std::env::temp_dir().join("mmvp-test-cache");
    let voices = Voices::open(Source::new(&src), &cache).unwrap();
    let index = |pkg: &str| std::fs::read(root.join(pkg).join("index")).unwrap();
    let originals = [index("AssetPackages"), index("AssetPackages\\Starter")];
    assert!(matches!(install.status().unwrap().state, State::Original));

    let t = std::time::Instant::now();
    let (bundles, clips) = install.apply(&voices, &|_, _, _| {}).unwrap();
    eprintln!("applied {bundles} bundles / {clips} clips in {:.1}s", t.elapsed().as_secs_f64());
    let st = install.status().unwrap();
    assert!(matches!(st.state, State::Patched));
    assert_eq!(st.voice.as_ref().map(|v| v.0.as_str()), Some(voices.id()));
    assert!(!st.stale);
    assert!(root.join("AssetPackages\\9876.blob").exists());

    install.apply(&voices, &|_, _, _| {}).unwrap(); // re-apply rewrites 9876.blob in place
    assert!(root.join("AssetPackages\\9876.blob").exists() && !root.join("AssetPackages\\9877.blob").exists());

    if std::env::var("MMVP_TEST_KEEP").is_ok() {
        return; // leave the patched copy for inspection
    }
    install.restore().unwrap();
    assert_eq!([index("AssetPackages"), index("AssetPackages\\Starter")], originals);
    assert!(matches!(install.status().unwrap().state, State::Original));
    assert!(!root.join("AssetPackages\\9876.blob").exists());
}
