//! Self-update from GitHub Releases. A release carries `mmvp.exe`, its signature `mmvp.exe.sig`
//! (Tauri / minisign format) and `latest.json` = {"version": "x.y.z"}. A newer exe is downloaded,
//! checked against the public key built in, swapped for the running one, and started.
//!
//! Build-time settings (release workflow): MMVP_REPO = GitHub OWNER/REPO,
//! MMVP_UPDATER_PUBKEY = the signing key's public key (base64, as printed by `tauri signer`).
//! Testing: MMVP_UPDATE_BASE = a directory URL holding latest.json, mmvp.exe and mmvp.exe.sig.
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use mmvp_shared::net;
use std::fs;

const EXE: &str = "mmvp.exe";

/// (latest.json URL, URL of the exe of a version)
fn urls(version: &str) -> Result<(String, String)> {
    if let Ok(base) = std::env::var("MMVP_UPDATE_BASE") {
        let base = base.trim_end_matches('/');
        return Ok((format!("{base}/latest.json"), format!("{base}/{EXE}")));
    }
    let repo = option_env!("MMVP_REPO").ok_or_else(|| anyhow!("這個版本沒有設定更新來源"))?;
    Ok((
        format!("https://github.com/{repo}/releases/latest/download/latest.json"),
        format!("https://github.com/{repo}/releases/download/v{version}/{EXE}"),
    ))
}

fn parse(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Newer version on GitHub, if any.
pub fn check() -> Result<Option<String>> {
    let (latest, _) = urls("")?;
    let v: serde_json::Value = serde_json::from_slice(&net::get(&latest, &|_, _| {})?)?;
    let version = v.get("version").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("latest.json 格式錯誤"))?;
    Ok((parse(version) > parse(env!("CARGO_PKG_VERSION"))).then(|| version.to_string()))
}

/// Download `version`, verify, replace the running exe and start the new one. Returns only on error.
pub fn install(version: &str, progress: &dyn Fn(u64, u64)) -> Result<()> {
    let (_, url) = urls(version)?;
    let sig = net::get(&format!("{url}.sig"), &|_, _| {})?;
    let new = net::get(&url, progress)?;
    verify(&new, &sig)?;
    let exe = std::env::current_exe()?;
    let (tmp, old) = (exe.with_extension("exe.new"), exe.with_extension("exe.old"));
    fs::write(&tmp, &new)?;
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).map_err(|e| anyhow!("無法替換程式（資料夾不可寫入？）：{e}"))?;
    if let Err(e) = fs::rename(&tmp, &exe) {
        let _ = fs::rename(&old, &exe);
        bail!("無法替換程式：{e}");
    }
    std::process::Command::new(&exe).spawn()?;
    std::process::exit(0);
}

/// `<exe>.old` left by an update; the old process may still be exiting.
pub fn cleanup_old() {
    let Ok(old) = std::env::current_exe().map(|e| e.with_extension("exe.old")) else { return };
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

/// Tauri signature: base64 of a minisign signature, checked with the built-in public key.
fn verify(data: &[u8], sig: &[u8]) -> Result<()> {
    let b64 = |s: &str| -> Result<String> { Ok(String::from_utf8(base64::engine::general_purpose::STANDARD.decode(s.trim())?)?) };
    let key = option_env!("MMVP_UPDATER_PUBKEY").filter(|k| !k.is_empty()).ok_or_else(|| anyhow!("這個版本沒有設定更新公鑰"))?;
    let pk = minisign_verify::PublicKey::decode(&b64(key)?).map_err(|e| anyhow!("公鑰格式錯誤：{e}"))?;
    let sig = minisign_verify::Signature::decode(&b64(&String::from_utf8_lossy(sig))?).map_err(|e| anyhow!("簽章格式錯誤：{e}"))?;
    pk.verify(data, &sig, true).map_err(|e| anyhow!("簽章驗證失敗：{e}"))
}
