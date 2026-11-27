//! mmvp: apply / restore JP voices on a TW Mabinogi Mobile install.
//!
//! Build-time settings (set by the release workflow): MMVP_SOURCE = voice site URL, MMVP_REPO =
//! GitHub OWNER/REPO. MMVP_SOURCE can be overridden at run time (a URL or a local dump directory).
mod locate;
mod net;
pub mod patcher;
mod update;

use mmvp_shared::site::Latest;
use patcher::{Applied, Install, Source, Status, Voices};
use serde::Serialize;
use std::sync::{Mutex, MutexGuard, TryLockError};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Run blocking work off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

fn source() -> Res<Source> {
    let s = std::env::var("MMVP_SOURCE").ok().or_else(|| option_env!("MMVP_SOURCE").map(String::from));
    s.filter(|s| !s.is_empty()).map(|s| Source::new(&s)).ok_or_else(|| "這個版本沒有設定語音來源".into())
}

fn install() -> Res<Install> {
    let root = locate::root().ok_or("找不到瑪奇 Mobile（台版），請手動選擇遊戲目錄")?;
    Install::open(&root).map_err(err)
}

/// Only one apply / restore / update at a time: hold the guard for the duration.
fn busy() -> Res<MutexGuard<'static, ()>> {
    static BUSY: Mutex<()> = Mutex::new(());
    match BUSY.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(e)) => Ok(e.into_inner()), // the previous holder panicked: not busy
        Err(TryLockError::WouldBlock) => Err("另一個作業正在進行中".into()),
    }
}

#[derive(Clone, Serialize)]
struct Progress<S> {
    step: S,
    done: u64,
    total: u64,
}

fn emit(app: &AppHandle, step: impl Serialize + Clone, done: u64, total: u64) {
    let _ = app.emit("progress", Progress { step, done, total });
}

#[derive(Serialize)]
struct View {
    version: &'static str,
    root: Option<String>,
    game_running: bool,
    status: Option<Status>,
    error: Option<String>,
}

#[tauri::command]
async fn state() -> Res<View> {
    blocking(|| {
        let (root, game_running, status) = match install() {
            Ok(i) => (Some(i.root.display().to_string()), i.game_running(), i.status().map_err(err)),
            Err(e) => (None, false, Err(e)),
        };
        let error = status.as_ref().err().cloned();
        Ok(View { version: env!("CARGO_PKG_VERSION"), root, game_running, status: status.ok(), error })
    })
    .await
}

#[tauri::command]
async fn pick_root(app: AppHandle) -> Res<bool> {
    blocking(move || {
        let Some(p) = app.dialog().file().set_title("選擇瑪奇 Mobile（台版）遊戲目錄").blocking_pick_folder() else { return Ok(false) };
        locate::set_root(&locate::normalize(&p.into_path().map_err(err)?).map_err(err)?);
        Ok(true)
    })
    .await
}

#[tauri::command]
async fn voices() -> Res<Latest> {
    blocking(|| source()?.latest().map_err(err)).await
}

#[tauri::command]
async fn apply(app: AppHandle) -> Res<Applied> {
    blocking(move || {
        let _busy = busy()?;
        let voices = Voices::open(source()?, &locate::data_dir().join("voices")).map_err(err)?;
        install()?.apply(&voices, &|step, done, total| emit(&app, step, done, total)).map_err(err)
    })
    .await
}

#[tauri::command]
async fn restore() -> Res<()> {
    blocking(|| {
        let _busy = busy()?;
        install()?.restore().map_err(err)
    })
    .await
}

#[tauri::command]
async fn app_update() -> Res<Option<String>> {
    blocking(|| update::check().map_err(err)).await
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Res<()> {
    blocking(move || {
        let _busy = busy()?;
        update::install(&|done, total| emit(&app, "app", done, total)).map_err(err)
    })
    .await
}

pub fn run() {
    update::cleanup_old();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![state, pick_root, voices, apply, restore, app_update, install_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
