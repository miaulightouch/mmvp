#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! GUI patcher: pick the TW install, apply / restore JP voices downloaded from the voice site
//! (Cloudflare Pages), tell when a re-apply is needed, and update itself from GitHub Releases.
//!
//! Build-time configuration (set by the release workflow, nothing secret):
//!   MMVOICE_SOURCE   voice site base URL, e.g. https://mmvoice-jp.pages.dev
//! Runtime override for testing: environment variable MMVOICE_SOURCE (URL or local directory).
use anyhow::{anyhow, Result};
use mmv_core::patcher::{self, Event, Options, PkgState};
use mmv_core::voices::{self, Latest, Source, Voices};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

// ------------------------------------------------------------------ settings & locations
#[derive(Serialize, Deserialize, Default, Clone)]
struct Settings {
    root: Option<PathBuf>,
}

fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("mmvoice")
}
fn load_settings() -> Settings {
    fs::read(data_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save_settings(s: &Settings) -> Result<()> {
    fs::create_dir_all(data_dir())?;
    fs::write(data_dir().join("settings.json"), serde_json::to_vec_pretty(s)?)?;
    Ok(())
}

fn source() -> Option<Source> {
    let s = std::env::var("MMVOICE_SOURCE").ok().filter(|s| !s.is_empty())
        .or_else(|| option_env!("MMVOICE_SOURCE").filter(|s| !s.is_empty()).map(String::from))?;
    Some(Source::parse(&s))
}

/// Accept the install root itself or its parent (…\MabinogiMobile_TW -> …\MabinogiMobile_TW\appdata).
fn normalize_root(p: &Path) -> Option<PathBuf> {
    for c in [p.to_path_buf(), p.join("appdata")] {
        if c.join("AssetPackages").join("index").exists() || c.join("AssetPackages").join("index.bak").exists() {
            return Some(c);
        }
    }
    None
}

fn detect_root() -> Option<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hives = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];
    for (hive, path) in hives {
        let Ok(k) = RegKey::predef(hive).open_subkey(path) else { continue };
        for name in k.enum_keys().flatten() {
            let Ok(sub) = k.open_subkey(&name) else { continue };
            let Ok(loc) = sub.get_value::<String, _>("InstallLocation") else { continue };
            if loc.is_empty() {
                continue;
            }
            if let Some(r) = normalize_root(Path::new(loc.trim_matches('"'))) {
                if r.join(patcher::GAME_EXE).exists() {
                    return Some(r);
                }
            }
        }
    }
    for c in [r"D:\MabinogiMobile_TW", r"C:\MabinogiMobile_TW", r"C:\Program Files\MabinogiMobile_TW", r"C:\Program Files (x86)\MabinogiMobile_TW"] {
        if let Some(r) = normalize_root(Path::new(c)) {
            return Some(r);
        }
    }
    None
}

// ------------------------------------------------------------------ app state
struct AppState {
    busy: AtomicBool,
    voices: Mutex<Option<Arc<Voices>>>,
}

struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
fn begin(st: &AppState) -> Result<BusyGuard<'_>, String> {
    if st.busy.swap(true, Ordering::SeqCst) {
        return Err("另一個作業正在進行中".into());
    }
    Ok(BusyGuard(&st.busy))
}

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

/// Latest voices (manifest downloaded once per version and kept in memory).
fn voices(st: &AppState) -> Result<Arc<Voices>> {
    let src = source().ok_or_else(|| anyhow!("這個版本沒有設定語音來源"))?;
    let latest = voices::fetch_latest(&src).ok();
    let mut g = st.voices.lock().unwrap();
    if let Some(v) = g.as_ref() {
        if latest.as_ref().map_or(true, |l| l.manifest_sha1 == v.id) {
            return Ok(v.clone());
        }
    }
    let v = Arc::new(Voices::open(src, &data_dir().join("voices"))?);
    *g = Some(v.clone());
    Ok(v)
}

// ------------------------------------------------------------------ view (local only, fast)
#[derive(Serialize)]
struct PkgView {
    pkg: String,
    state: PkgState,
    redirected: usize,
}

#[derive(Serialize)]
struct View {
    version: &'static str,
    source: Option<String>,
    root: Option<String>,
    root_error: Option<String>,
    game_running: bool,
    pkgs: Vec<PkgView>,
    /// "original" | "patched" | "mixed" (packages patched with different voice versions or unknown)
    /// | "stale" (game files changed since apply) | "reverted" (index replaced) | "missing" | "unknown"
    overall: String,
    applied_id: Option<String>,
    applied_date: Option<String>,
    stale: Vec<String>,
}

#[tauri::command]
fn get_view() -> Result<View, String> {
    let mut s = load_settings();
    if s.root.as_ref().map_or(true, |r| normalize_root(r).is_none()) {
        if let Some(r) = detect_root() {
            s.root = Some(r);
            let _ = save_settings(&s);
        }
    }
    let mut v = View {
        version: env!("CARGO_PKG_VERSION"),
        source: source().map(|s| s.describe()),
        root: s.root.as_ref().map(|p| p.display().to_string()),
        root_error: None,
        game_running: false,
        pkgs: Vec::new(),
        overall: "unknown".into(),
        applied_id: None,
        applied_date: None,
        stale: Vec::new(),
    };
    let Some(root) = s.root.as_deref() else {
        v.root_error = Some("找不到瑪奇 Mobile（台版）安裝位置，請手動選擇".into());
        return Ok(v);
    };
    v.game_running = patcher::game_running(root);
    let list = match patcher::status(root) {
        Ok(l) => l,
        Err(e) => {
            v.root_error = Some(err(e));
            return Ok(v);
        }
    };
    let (mut missing, mut reverted, mut patched, mut mixed) = (false, false, false, false);
    for p in list {
        match &p.state {
            PkgState::Missing => missing = true,
            PkgState::Reverted => reverted = true,
            PkgState::Patched { pack_id, pack_created, stale, .. } => {
                patched = true;
                v.stale.extend(stale.iter().cloned());
                match pack_id {
                    None => mixed = true,
                    Some(id) => {
                        if v.applied_id.as_ref().map_or(false, |a| a != id) {
                            mixed = true;
                        }
                        v.applied_id = Some(id.clone());
                        v.applied_date = pack_created.clone();
                    }
                }
            }
            PkgState::Original => {}
        }
        v.pkgs.push(PkgView { pkg: p.pkg, state: p.state, redirected: p.entries_redirected });
    }
    v.overall = if missing {
        "missing"
    } else if reverted {
        "reverted"
    } else if !v.stale.is_empty() {
        "stale"
    } else if patched && mixed {
        "mixed"
    } else if patched {
        "patched"
    } else {
        "original"
    }
    .into();
    Ok(v)
}

#[tauri::command]
fn pick_root(app: AppHandle) -> Result<Option<String>, String> {
    let Some(p) = app.dialog().file().set_title("選擇瑪奇 Mobile（台版）安裝資料夾").blocking_pick_folder() else { return Ok(None) };
    let p = p.into_path().map_err(err)?;
    let root = normalize_root(&p).ok_or("這個資料夾不是瑪奇 Mobile 的安裝目錄（找不到 AssetPackages\\index）")?;
    let mut s = load_settings();
    s.root = Some(root.clone());
    save_settings(&s).map_err(err)?;
    Ok(Some(root.display().to_string()))
}

// ------------------------------------------------------------------ voices (network)
#[derive(Serialize)]
struct VoicesInfo {
    date: String,
    id: String,
    clips: usize,
    bytes: u64,
}

#[tauri::command]
async fn check_voices(app: AppHandle) -> Result<VoicesInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let v = voices(&app.state::<AppState>()).map_err(err)?;
        let Latest { date, clips, bytes, .. } = v.latest.clone();
        Ok(VoicesInfo { date, id: v.id.clone(), clips, bytes })
    })
    .await
    .map_err(err)?
}

// ------------------------------------------------------------------ patch / restore
#[derive(Clone, Serialize)]
struct Progress {
    stage: String,
    done: usize,
    total: usize,
}
#[derive(Clone, Serialize)]
struct Log {
    level: &'static str,
    msg: String,
}

fn reporter(app: &AppHandle) -> impl Fn(Event) + Sync + '_ {
    move |e| {
        let _ = match e {
            Event::Stage(s) => app.emit("progress", Progress { stage: s.into(), done: 0, total: 0 }),
            Event::Progress { stage, done, total } => app.emit("progress", Progress { stage: stage.into(), done, total }),
            Event::Warn(m) => app.emit("log", Log { level: "warn", msg: m }),
            Event::Info(m) => app.emit("log", Log { level: "info", msg: m }),
        };
    }
}

#[derive(Serialize)]
struct PatchResult {
    bundles: usize,
    clips: usize,
}

#[tauri::command]
async fn patch(app: AppHandle, dry_run: bool) -> Result<PatchResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let _g = begin(&st)?;
        let root = load_settings().root.ok_or("尚未選擇安裝目錄")?;
        let v = voices(&st).map_err(err)?;
        let cache = data_dir().join("scan_cache.json");
        let rep = reporter(&app);
        let r = patcher::patch(&Options { root: &root, voices: &v, cache: &cache, rescan: false, dry_run, blob_no: patcher::DEFAULT_BLOB_NO }, &rep)
            .map_err(err)?;
        Ok(PatchResult { bundles: r.bundles, clips: r.clips })
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn restore(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let _g = begin(&st)?;
        let root = load_settings().root.ok_or("尚未選擇安裝目錄")?;
        let rep = reporter(&app);
        patcher::restore(&root, &rep).map_err(err)
    })
    .await
    .map_err(err)?
}

// ------------------------------------------------------------------ self update (GitHub Releases)
#[derive(Serialize)]
struct AppUpdate {
    version: String,
    notes: Option<String>,
}

#[tauri::command]
async fn check_app_update(app: AppHandle) -> Result<Option<AppUpdate>, String> {
    let u = app.updater().map_err(err)?.check().await.map_err(err)?;
    Ok(u.map(|u| AppUpdate { version: u.version.clone(), notes: u.body.clone() }))
}

#[tauri::command]
async fn install_app_update(app: AppHandle) -> Result<(), String> {
    let st = app.state::<AppState>();
    let _g = begin(&st)?;
    let Some(u) = app.updater().map_err(err)?.check().await.map_err(err)? else { return Ok(()) };
    let mut got = 0usize;
    u.download_and_install(
        |chunk, total| {
            got += chunk;
            let _ = app.emit("progress", Progress { stage: "app".into(), done: got >> 10, total: total.unwrap_or(0) as usize >> 10 });
        },
        || {},
    )
    .await
    .map_err(err)?;
    app.restart();
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState { busy: AtomicBool::new(false), voices: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![get_view, pick_root, check_voices, patch, restore, check_app_update, install_app_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
