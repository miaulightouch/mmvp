#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! GUI installer for the JP voice pack: pick the TW install, apply / restore, detect game updates,
//! check for and download newer packs.
use anyhow::{anyhow, bail, Context, Result};
use mmv_core::pack::Pack;
use mmv_core::patcher::{self, Event, Options, PkgState};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

// ------------------------------------------------------------------ settings & locations
#[derive(Serialize, Deserialize, Default, Clone)]
struct Settings {
    root: Option<PathBuf>,
    pack: Option<PathBuf>,
    /// pack id (sha1 of its manifest) last applied successfully
    applied_pack: Option<String>,
}

/// Optional `mmvoice.json` next to the exe, written by whoever distributes the installer.
#[derive(Deserialize, Default)]
struct Portable {
    update_url: Option<String>,
}

fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("mmvoice")
}
fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default()
}
fn load_settings() -> Settings {
    fs::read(data_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save_settings(s: &Settings) -> Result<()> {
    fs::create_dir_all(data_dir())?;
    fs::write(data_dir().join("settings.json"), serde_json::to_vec_pretty(s)?)?;
    Ok(())
}
fn portable() -> Portable {
    fs::read(exe_dir().join("mmvoice.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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

/// Pack to use: chosen one, else the newest *.mmvpack next to the exe or in the data dir.
fn find_pack(s: &Settings) -> Option<PathBuf> {
    if let Some(p) = &s.pack {
        if p.exists() {
            return Some(p.clone());
        }
    }
    let mut c: Vec<(std::time::SystemTime, PathBuf)> = [exe_dir(), data_dir()]
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |x| x == "mmvpack"))
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    c.sort();
    c.pop().map(|x| x.1)
}

// ------------------------------------------------------------------ app state
struct AppState {
    busy: AtomicBool,
    pack: Mutex<Option<(PathBuf, std::sync::Arc<Pack>)>>,
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

fn load_pack(st: &AppState, path: &Path) -> Result<std::sync::Arc<Pack>> {
    let mut g = st.pack.lock().unwrap();
    if let Some((p, pk)) = g.as_ref() {
        if p == path {
            return Ok(pk.clone());
        }
    }
    let pk = std::sync::Arc::new(Pack::open(path)?);
    *g = Some((path.to_path_buf(), pk.clone()));
    Ok(pk)
}

#[derive(Serialize)]
struct PackInfo {
    path: String,
    id: String,
    created: String,
    clips: usize,
}

#[derive(Serialize)]
struct PkgView {
    pkg: String,
    state: String,
    blob: Option<u16>,
    redirected: usize,
}

#[derive(Serialize)]
struct View {
    version: &'static str,
    root: Option<String>,
    root_error: Option<String>,
    game_running: bool,
    pkgs: Vec<PkgView>,
    /// "original" | "patched" | "reverted" | "partial" | "missing" | "unknown"
    overall: String,
    pack: Option<PackInfo>,
    pack_error: Option<String>,
    applied_pack: Option<String>,
    update_url: Option<String>,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn get_view(st: State<AppState>) -> Result<View, String> {
    let mut s = load_settings();
    if s.root.as_ref().map_or(true, |r| normalize_root(r).is_none()) {
        if let Some(r) = detect_root() {
            s.root = Some(r);
            let _ = save_settings(&s);
        }
    }
    let mut v = View {
        version: env!("CARGO_PKG_VERSION"),
        root: s.root.as_ref().map(|p| p.display().to_string()),
        root_error: None,
        game_running: false,
        pkgs: Vec::new(),
        overall: "unknown".into(),
        pack: None,
        pack_error: None,
        applied_pack: s.applied_pack.clone(),
        update_url: portable().update_url,
    };
    match s.root.as_deref() {
        None => v.root_error = Some("找不到瑪奇 Mobile（台版）安裝位置，請手動選擇".into()),
        Some(root) => {
            v.game_running = patcher::game_running(root);
            match patcher::status(root) {
                Ok(list) => {
                    let mut states = Vec::new();
                    for p in list {
                        let (state, blob) = match p.state {
                            PkgState::Original => ("original", None),
                            PkgState::Patched(n) => ("patched", Some(n)),
                            PkgState::Reverted => ("reverted", None),
                            PkgState::Missing => ("missing", None),
                        };
                        states.push(state);
                        v.pkgs.push(PkgView { pkg: p.pkg, state: state.into(), blob, redirected: p.entries_redirected });
                    }
                    v.overall = if states.contains(&"missing") {
                        "missing"
                    } else if states.contains(&"reverted") {
                        "reverted"
                    } else if states.contains(&"patched") {
                        "patched"
                    } else {
                        "original"
                    }
                    .into();
                }
                Err(e) => v.root_error = Some(err(e)),
            }
        }
    }
    match find_pack(&s) {
        None => v.pack_error = Some("找不到語音包（.mmvpack），請選擇檔案或檢查更新".into()),
        Some(p) => match load_pack(&st, &p) {
            Ok(pk) => v.pack = Some(PackInfo { path: p.display().to_string(), id: pk.id.clone(), created: pk.manifest.created.clone(), clips: pk.manifest.clips.len() }),
            Err(e) => v.pack_error = Some(err(e)),
        },
    }
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

#[tauri::command]
fn pick_pack(app: AppHandle, st: State<AppState>) -> Result<Option<String>, String> {
    let Some(p) = app.dialog().file().set_title("選擇語音包").add_filter("語音包", &["mmvpack"]).blocking_pick_file() else { return Ok(None) };
    let p = p.into_path().map_err(err)?;
    load_pack(&st, &p).map_err(err)?;
    let mut s = load_settings();
    s.pack = Some(p.clone());
    save_settings(&s).map_err(err)?;
    Ok(Some(p.display().to_string()))
}

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
    blobs: usize,
    bundles: usize,
    clips: usize,
}

fn do_patch(app: &AppHandle, st: &AppState, dry_run: bool) -> Result<PatchResult> {
    let mut s = load_settings();
    let root = s.root.clone().ok_or_else(|| anyhow!("尚未選擇安裝目錄"))?;
    let pack_path = find_pack(&s).ok_or_else(|| anyhow!("找不到語音包"))?;
    let pack = load_pack(st, &pack_path)?;
    let cache = data_dir().join("scan_cache.json");
    let rep = reporter(app);
    let r = patcher::patch(&Options { root: &root, pack: &pack, cache: &cache, rescan: false, dry_run, blob_no: patcher::DEFAULT_BLOB_NO }, &rep)?;
    if !dry_run {
        s.applied_pack = Some(pack.id.clone());
        save_settings(&s)?;
    }
    Ok(PatchResult { blobs: r.blobs, bundles: r.bundles, clips: r.clips })
}

#[tauri::command]
async fn patch(app: AppHandle, dry_run: bool) -> Result<PatchResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let _g = begin(&st)?;
        do_patch(&app, &st, dry_run).map_err(err)
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
        patcher::restore(&root, &rep).map_err(err)?;
        let mut s = load_settings();
        s.applied_pack = None;
        save_settings(&s).map_err(err)
    })
    .await
    .map_err(err)?
}

// ------------------------------------------------------------------ updates
/// update.json: {"pack": {"id", "created", "url", "size", "sha1"}, "app": {"version", "url"}}
#[derive(Deserialize, Serialize, Clone)]
struct RemotePack {
    id: String,
    #[serde(default)]
    created: String,
    url: String,
    size: u64,
    sha1: String,
}
#[derive(Deserialize, Serialize, Clone)]
struct RemoteApp {
    version: String,
    url: String,
}
#[derive(Deserialize, Serialize, Clone)]
struct Remote {
    pack: Option<RemotePack>,
    app: Option<RemoteApp>,
}

fn version_newer(a: &str, b: &str) -> bool {
    let p = |s: &str| s.trim_start_matches('v').split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    p(a) > p(b)
}

#[derive(Serialize)]
struct UpdateInfo {
    pack: Option<RemotePack>,
    pack_newer: bool,
    app: Option<RemoteApp>,
    app_newer: bool,
}

#[tauri::command]
async fn check_update(st: State<'_, AppState>) -> Result<UpdateInfo, String> {
    let url = portable().update_url.ok_or("沒有設定更新網址（mmvoice.json 的 update_url）")?;
    let remote: Remote = tauri::async_runtime::spawn_blocking(move || -> Result<Remote> {
        Ok(ureq::get(&url).timeout(std::time::Duration::from_secs(15)).call()?.into_json()?)
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    let current = find_pack(&load_settings()).and_then(|p| load_pack(&st, &p).ok()).map(|p| p.id.clone());
    Ok(UpdateInfo {
        pack_newer: remote.pack.as_ref().map_or(false, |r| Some(&r.id) != current.as_ref()),
        app_newer: remote.app.as_ref().map_or(false, |r| version_newer(&r.version, env!("CARGO_PKG_VERSION"))),
        pack: remote.pack,
        app: remote.app,
    })
}

fn download(app: &AppHandle, rp: &RemotePack) -> Result<PathBuf> {
    fs::create_dir_all(data_dir())?;
    let dst = data_dir().join(format!("jp_voice_{}.mmvpack", &rp.id[..rp.id.len().min(12)]));
    let tmp = dst.with_extension("part");
    let resp = ureq::get(&rp.url).call().context("下載失敗")?;
    let mut r = resp.into_reader();
    let mut f = fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut hasher = mmv_core::audio::Sha1Stream::new();
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        let _ = app.emit("progress", Progress { stage: "download".into(), done: (done >> 10) as usize, total: (rp.size >> 10) as usize });
    }
    drop(f);
    if hasher.hex() != rp.sha1.to_lowercase() {
        let _ = fs::remove_file(&tmp);
        bail!("下載的檔案 SHA1 不符，請重試");
    }
    fs::rename(&tmp, &dst)?;
    let pk = Pack::open(&dst)?;
    if pk.id != rp.id {
        bail!("下載的語音包內容與更新資訊不符");
    }
    Ok(dst)
}

#[tauri::command]
async fn download_pack(app: AppHandle, pack: RemotePack) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let _g = begin(&st)?;
        let p = download(&app, &pack).map_err(err)?;
        load_pack(&st, &p).map_err(err)?;
        let mut s = load_settings();
        s.pack = Some(p.clone());
        save_settings(&s).map_err(err)?;
        Ok(p.display().to_string())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if !url.starts_with("https://") {
        return Err("只開啟 https 連結".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(err)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { busy: AtomicBool::new(false), pack: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![get_view, pick_root, pick_pack, patch, restore, check_update, download_pack, open_url])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
