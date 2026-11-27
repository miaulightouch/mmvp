//! Self-update from GitHub Releases. A release carries `mmvp.exe`, its signature `mmvp.exe.sig`
//! (Tauri / minisign format) and `latest.json` = {"version": "x.y.z"}. When the latest release is
//! newer, its exe is downloaded, checked against the public key built in, swapped for the running
//! one, and started.
//!
//! Build-time settings (release workflow): MMVP_REPO = GitHub OWNER/REPO,
//! MMVP_UPDATER_PUBKEY = the signing key's public key (base64, as printed by `tauri signer`).
//! Testing: MMVP_UPDATE_BASE = a directory URL holding latest.json, mmvp.exe and mmvp.exe.sig.
use crate::net;
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use std::fs;

const EXE: &str = "mmvp.exe";

/// URL of a file of the latest release.
fn url(file: &str) -> Result<String> {
    if let Ok(base) = std::env::var("MMVP_UPDATE_BASE") {
        return Ok(format!("{}/{file}", base.trim_end_matches('/')));
    }
    let repo = option_env!("MMVP_REPO").ok_or_else(|| anyhow!(tr!("No update source is configured in this build", "這個版本沒有設定更新來源")))?;
    Ok(format!("https://github.com/{repo}/releases/latest/download/{file}"))
}

/// "x.y.z" as a comparable key.
fn version_key(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Version of the latest release, if newer than this build.
pub fn check() -> Result<Option<String>> {
    let v: serde_json::Value = serde_json::from_slice(&net::get(&url("latest.json")?, &|_, _| {})?)?;
    let version = v.get("version").and_then(|v| v.as_str()).ok_or_else(|| anyhow!(tr!("latest.json is malformed", "latest.json 格式錯誤")))?;
    Ok((version_key(version) > version_key(env!("CARGO_PKG_VERSION"))).then(|| version.to_string()))
}

/// Download the latest exe, verify it, replace the running exe and start the new one. Returns only on error.
pub fn install(progress: &dyn Fn(u64, u64)) -> Result<()> {
    let sig = net::get(&url(&format!("{EXE}.sig"))?, &|_, _| {})?;
    let new = net::get(&url(EXE)?, progress)?;
    verify(&new, &sig)?;
    let exe = std::env::current_exe()?;
    let (tmp, old) = (exe.with_extension("exe.new"), exe.with_extension("exe.old"));
    fs::write(&tmp, &new)?;
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).map_err(|e| anyhow!(tr!("Cannot replace the program (folder not writable?): {e}", "無法替換程式（資料夾不可寫入？）：{e}")))?;
    if let Err(e) = fs::rename(&tmp, &exe) {
        let _ = fs::rename(&old, &exe);
        bail!(tr!("Cannot replace the program: {e}", "無法替換程式：{e}"));
    }
    std::process::Command::new(&exe).spawn()?;
    std::process::exit(0);
}

/// Delete the `<exe>.old` an update left behind. If the old process is still exiting, it stays until
/// the next start.
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(exe.with_extension("exe.old"));
    }
}

/// Tauri signature: base64 of a minisign signature, checked with the built-in public key.
fn verify(data: &[u8], sig: &[u8]) -> Result<()> {
    let b64 = |s: &str| -> Result<String> { Ok(String::from_utf8(base64::engine::general_purpose::STANDARD.decode(s.trim())?)?) };
    let key = option_env!("MMVP_UPDATER_PUBKEY").filter(|k| !k.is_empty()).ok_or_else(|| anyhow!(tr!("No update key is configured in this build", "這個版本沒有設定更新公鑰")))?;
    let pk = minisign_verify::PublicKey::decode(&b64(key)?).map_err(|e| anyhow!(tr!("Bad public key: {e}", "公鑰格式錯誤：{e}")))?;
    let sig = minisign_verify::Signature::decode(&b64(&String::from_utf8_lossy(sig))?).map_err(|e| anyhow!(tr!("Bad signature: {e}", "簽章格式錯誤：{e}")))?;
    pk.verify(data, &sig, true).map_err(|e| anyhow!(tr!("Signature check failed: {e}", "簽章驗證失敗：{e}")))
}
