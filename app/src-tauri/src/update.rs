//! Self-update from GitHub Releases. The release's latest.json (Tauri updater) says which version is
//! current. An installed copy (NSIS, uninstall.exe next to it) lets the updater run the new setup; a
//! portable copy downloads `mmvp-portable.exe` from the same release, checks its signature with the
//! updater public key, swaps itself and restarts.
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

pub fn is_portable() -> bool {
    let exe = std::env::current_exe().unwrap_or_default();
    !exe.parent().is_some_and(|d| d.join("uninstall.exe").exists())
}

/// `<exe>.old` left behind by a portable update; the old process may still be exiting.
pub fn cleanup_old() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = exe.with_extension("exe.old");
    if old.exists() {
        std::thread::spawn(move || {
            for _ in 0..30 {
                if fs::remove_file(&old).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    }
}

pub async fn check(app: &AppHandle) -> Result<Option<String>> {
    Ok(app.updater()?.check().await?.map(|u| u.version))
}

pub async fn install(app: &AppHandle, progress: impl Fn(u64, u64) + Send + Sync + 'static) -> Result<()> {
    let Some(u) = app.updater()?.check().await? else { return Ok(()) };
    if is_portable() {
        let app = app.clone();
        return tauri::async_runtime::spawn_blocking(move || portable(&app, &u.version, &progress)).await?;
    }
    let mut got = 0u64;
    u.download_and_install(|n, total| { got += n as u64; progress(got, total.unwrap_or(0)) }, || {}).await?;
    app.restart();
}

fn portable(app: &AppHandle, version: &str, progress: &dyn Fn(u64, u64)) -> Result<()> {
    let url = match std::env::var("MMVP_PORTABLE_BASE") {
        Ok(base) => format!("{}/mmvp-portable.exe", base.trim_end_matches('/')), // testing
        Err(_) => {
            let repo = option_env!("MMVP_REPO").ok_or_else(|| anyhow!("這個版本沒有設定更新來源"))?;
            format!("https://github.com/{repo}/releases/download/v{version}/mmvp-portable.exe")
        }
    };
    let sig = mmvp_core::net::get(&format!("{url}.sig"), &|_, _| {})?;
    let exe_bytes = mmvp_core::net::get(&url, progress)?;
    verify(app, &exe_bytes, &sig)?;
    let exe = std::env::current_exe()?;
    let (new, old): (PathBuf, PathBuf) = (exe.with_extension("exe.new"), exe.with_extension("exe.old"));
    fs::write(&new, &exe_bytes)?;
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).map_err(|e| anyhow!("無法替換程式（資料夾不可寫入？）：{e}"))?;
    if let Err(e) = fs::rename(&new, &exe) {
        let _ = fs::rename(&old, &exe);
        bail!("無法替換程式：{e}");
    }
    std::process::Command::new(&exe).spawn()?;
    app.exit(0);
    Ok(())
}

/// Tauri updater signature: base64 of a minisign signature, checked with the configured public key.
fn verify(app: &AppHandle, data: &[u8], sig: &[u8]) -> Result<()> {
    let b64 = |s: &str| -> Result<String> { Ok(String::from_utf8(base64::engine::general_purpose::STANDARD.decode(s.trim())?)?) };
    let pubkey = app.config().plugins.0.get("updater").and_then(|u| u.get("pubkey")).and_then(|p| p.as_str()).unwrap_or_default();
    if pubkey.is_empty() {
        bail!("這個版本沒有設定更新公鑰");
    }
    let pk = minisign_verify::PublicKey::decode(&b64(pubkey)?).map_err(|e| anyhow!("公鑰格式錯誤：{e}"))?;
    let sig = minisign_verify::Signature::decode(&b64(&String::from_utf8_lossy(sig))?).map_err(|e| anyhow!("簽章格式錯誤：{e}"))?;
    pk.verify(data, &sig, true).map_err(|e| anyhow!("簽章驗證失敗：{e}"))
}
