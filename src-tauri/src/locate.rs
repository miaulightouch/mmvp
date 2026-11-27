//! Where things are: our data directory, the remembered game folder, and the TW install recorded by
//! the Nexon Launcher.
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("mmvp")
}

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    root: Option<PathBuf>,
}

/// Accept the install root itself or its parent (…\MabinogiMobile_TW -> …\MabinogiMobile_TW\appdata).
pub fn normalize(p: &Path) -> Option<PathBuf> {
    [p.to_path_buf(), p.join("appdata")].into_iter().find(|c| crate::patcher::Install::open(c).is_ok())
}

/// [確認] The TW client is installed by the Nexon Launcher, which records it in
/// %APPDATA%\NexonLauncher\installed-apps.db (JSON) as product 59826 "MabinogiMobile_TW".
/// The JP client is product 2943 and must never be picked.
fn nexon_launcher() -> Option<PathBuf> {
    let db = PathBuf::from(std::env::var_os("APPDATA")?).join("NexonLauncher").join("installed-apps.db");
    let v: serde_json::Value = serde_json::from_slice(&fs::read(db).ok()?).ok()?;
    let apps = v.get("installedApps")?.as_object()?;
    let tw = apps.get("59826").or_else(|| apps.values().find(|a| a.get("name").and_then(|n| n.as_str()) == Some("MabinogiMobile_TW")))?;
    normalize(Path::new(tw.get("installPath")?.as_str()?))
}

/// The remembered folder if still valid, else the Nexon Launcher's record (remembered from then on).
pub fn root() -> Option<PathBuf> {
    let path = data_dir().join("settings.json");
    let saved: Settings = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if let Some(r) = saved.root.as_deref().and_then(normalize) {
        return Some(r);
    }
    let found = nexon_launcher()?;
    set_root(&found);
    Some(found)
}

pub fn set_root(root: &Path) {
    let _ = fs::create_dir_all(data_dir());
    let s = Settings { root: Some(root.to_path_buf()) };
    let _ = fs::write(data_dir().join("settings.json"), serde_json::to_vec_pretty(&s).unwrap_or_default());
}
