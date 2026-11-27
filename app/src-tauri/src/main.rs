#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! mmvp: apply / restore JP voices on a TW Mabinogi Mobile install.
//!
//! Build-time settings (set by the release workflow): MMVP_SOURCE = voice site URL, MMVP_REPO =
//! GitHub OWNER/REPO. MMVP_SOURCE can be overridden at run time (a URL or a local dump directory).
mod locate;
mod update;

use mmvp_core::{Install, Source, Status, Step, Voices};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

fn source() -> Res<Source> {
    let s = std::env::var("MMVP_SOURCE").ok().or_else(|| option_env!("MMVP_SOURCE").map(String::from));
    s.filter(|s| !s.is_empty()).map(|s| Source::new(&s)).ok_or_else(|| "這個版本沒有設定語音來源".into())
}

fn install() -> Res<Install> {
    let root = locate::root().ok_or("找不到瑪奇 Mobile（台版），請手動選擇遊戲目錄")?;
    Install::open(&root).map_err(err)
}

/// Only one apply / restore / update at a time.
struct Busy(AtomicBool);
struct Guard<'a>(&'a AtomicBool);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
fn busy(app: &AppHandle) -> Res<Guard<'_>> {
    let b = &app.state::<Busy>().inner().0;
    if b.swap(true, Ordering::SeqCst) {
        return Err("另一個作業正在進行中".into());
    }
    Ok(Guard(b))
}

#[derive(Clone, Serialize)]
struct Progress {
    step: &'static str,
    done: u64,
    total: u64,
}

fn emit(app: &AppHandle, step: &'static str, done: u64, total: u64) {
    let _ = app.emit("progress", Progress { step, done, total });
}

#[derive(Serialize)]
struct View {
    version: &'static str,
    root: Option<String>,
    error: Option<String>,
    game_running: bool,
    status: Option<Status>,
}

#[tauri::command]
fn state() -> View {
    let mut v = View { version: env!("CARGO_PKG_VERSION"), root: None, error: None, game_running: false, status: None };
    match install() {
        Ok(i) => {
            v.root = Some(i.root.display().to_string());
            v.game_running = i.game_running();
            match i.status() {
                Ok(s) => v.status = Some(s),
                Err(e) => v.error = Some(err(e)),
            }
        }
        Err(e) => v.error = Some(e),
    }
    v
}

#[tauri::command]
fn pick_root(app: AppHandle) -> Res<bool> {
    let Some(p) = app.dialog().file().set_title("選擇瑪奇 Mobile（台版）遊戲目錄").blocking_pick_folder() else { return Ok(false) };
    let root = locate::normalize(&p.into_path().map_err(err)?).ok_or("這不是瑪奇 Mobile 的遊戲目錄（找不到 AssetPackages\\index）")?;
    locate::set_root(&root);
    Ok(true)
}

#[derive(Serialize)]
struct VoicesInfo {
    id: String,
    date: String,
    clips: usize,
    bytes: u64,
}

#[tauri::command]
async fn voices() -> Res<VoicesInfo> {
    let l = tauri::async_runtime::spawn_blocking(|| source()?.latest().map_err(err)).await.map_err(err)??;
    Ok(VoicesInfo { id: l.manifest_sha1, date: l.date, clips: l.clips, bytes: l.bytes })
}

#[tauri::command]
async fn apply(app: AppHandle) -> Res<(usize, usize)> {
    tauri::async_runtime::spawn_blocking(move || {
        let _g = busy(&app)?;
        let i = install()?;
        let v = Voices::open(source()?, &locate::data_dir().join("voices")).map_err(err)?;
        let progress = |s: Step, d, t| {
            let step = match s {
                Step::Scan => "scan",
                Step::Download => "download",
                Step::Rebuild => "rebuild",
                Step::Commit => "commit",
            };
            emit(&app, step, d, t);
        };
        i.apply(&v, &progress).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn restore(app: AppHandle) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _g = busy(&app)?;
        install()?.restore().map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn app_update(app: AppHandle) -> Res<Option<String>> {
    update::check(&app).await.map_err(err)
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Res<()> {
    let _g = busy(&app)?;
    let a = app.clone();
    update::install(&app, move |d, t| emit(&a, "app", d, t)).await.map_err(err)
}

fn main() {
    update::cleanup_old();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Busy(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![state, pick_root, voices, apply, restore, app_update, install_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
