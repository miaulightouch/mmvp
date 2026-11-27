//! mmvp: apply / restore JP voices on a TW Mabinogi Mobile install. The TW install is found through
//! the Nexon Launcher; a client of another region with the same layout can be picked by hand
//! (experimental, see locate::experimental).
//!
//! Build-time settings (set by the release workflow): MMVP_SOURCE = voice site URL, MMVP_REPO =
//! GitHub OWNER/REPO. MMVP_SOURCE can be overridden at run time (a URL or a local dump directory).
#[macro_use]
mod i18n;
mod locate;
mod net;
pub mod patcher;
mod update;

use mmvp_shared::site::Latest;
use patcher::{Applied, Install, Source, Status, Voices};
use serde::Serialize;
use std::sync::{Mutex, MutexGuard, TryLockError};
use tauri::{AppHandle, Emitter, Manager};
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
    s.filter(|s| !s.is_empty()).map(|s| Source::new(&s)).ok_or_else(|| tr!("No voice source is configured in this build", "這個版本沒有設定語音來源"))
}

fn install() -> Res<Install> {
    let root = locate::root().ok_or_else(|| tr!("Mabinogi Mobile (TW) was not found; choose the game folder", "找不到瑪奇 Mobile（台版），請手動選擇遊戲目錄"))?;
    Install::open(&root).map_err(err)
}

/// Only one apply / restore / update at a time: hold the guard for the duration.
fn busy() -> Res<MutexGuard<'static, ()>> {
    static BUSY: Mutex<()> = Mutex::new(());
    match BUSY.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(e)) => Ok(e.into_inner()), // the previous holder panicked: not busy
        Err(TryLockError::WouldBlock) => Err(tr!("Another task is running", "另一個作業正在進行中")),
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
    /// not the TW install the Nexon Launcher records (another region's client, or a copy)
    experimental: bool,
    game_running: bool,
    status: Option<Status>,
    error: Option<String>,
}

/// The front end reports its language (Traditional Chinese or English) before anything else.
#[tauri::command]
fn set_lang(app: AppHandle, zh: bool) {
    i18n::set_zh(zh);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_title(&tr!("Mabinogi Mobile JP Voice", "瑪奇 Mobile 日文語音"));
    }
}

#[tauri::command]
async fn state() -> Res<View> {
    blocking(|| {
        let (root, experimental, game_running, status) = match install() {
            Ok(i) => (Some(i.root.display().to_string()), locate::experimental(&i.root), i.game_running(), i.status().map_err(err)),
            Err(e) => (None, false, false, Err(e)),
        };
        let error = status.as_ref().err().cloned();
        Ok(View { version: env!("CARGO_PKG_VERSION"), root, experimental, game_running, status: status.ok(), error })
    })
    .await
}

#[tauri::command]
async fn pick_root(app: AppHandle) -> Res<bool> {
    blocking(move || {
        let title = tr!("Choose the Mabinogi Mobile game folder (clients other than TW are experimental)", "選擇瑪奇 Mobile 遊戲目錄（台版以外為實驗性支援）");
        let Some(p) = app.dialog().file().set_title(title).blocking_pick_folder() else { return Ok(false) };
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
        .invoke_handler(tauri::generate_handler![set_lang, state, pick_root, voices, apply, restore, app_update, install_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
