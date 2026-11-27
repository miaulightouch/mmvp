//! Where things are: our data directory, the game folder the user picked, and the TW install recorded
//! by the Nexon Launcher.
use crate::patcher::Install;
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
pub fn normalize(p: &Path) -> anyhow::Result<PathBuf> {
    let open = |c: &Path| Install::open(c).map(|i| i.root);
    open(p).or_else(|e| open(&p.join("appdata")).map_err(|_| e))
}

/// [確認] The TW client is installed by the Nexon Launcher, which records it in
/// %APPDATA%\NexonLauncher\installed-apps.db (JSON) as product 59826.
/// The JP client is product 2943 and must never be picked.
fn nexon_launcher() -> Option<PathBuf> {
    let db = PathBuf::from(std::env::var_os("APPDATA")?).join("NexonLauncher").join("installed-apps.db");
    let v: serde_json::Value = serde_json::from_slice(&fs::read(db).ok()?).ok()?;
    normalize(Path::new(v.get("installedApps")?.get("59826")?.get("installPath")?.as_str()?)).ok()
}

/// The folder the user picked if still valid, else the Nexon Launcher's record.
pub fn root() -> Option<PathBuf> {
    let saved: Settings = fs::read(data_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    saved.root.and_then(|r| normalize(&r).ok()).or_else(nexon_launcher)
}

pub fn set_root(root: &Path) {
    let _ = fs::create_dir_all(data_dir());
    let s = Settings { root: Some(root.to_path_buf()) };
    let _ = fs::write(data_dir().join("settings.json"), serde_json::to_vec_pretty(&s).unwrap_or_default());
}
