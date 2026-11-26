const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const STAGES = { scan: "掃描遊戲檔案", plan: "比對語音", download: "下載日文語音", rebuild: "重新打包", commit: "寫入", app: "下載新版程式" };
const STATE_TEXT = {
  original: "韓語（原版）", patched: "日文語音已套用", reverted: "遊戲更新後已還原為韓語",
  mixed: "已套用（版本不明）", stale: "已套用，但遊戲檔案已更新",
  missing: "index 不存在（launcher 更新中？）", unknown: "未知",
};

let view = null;
let voices = null;      // latest voice version on the site
let voicesError = null;
let appUpdate = null;
let busy = false;

function fmtDate(s) {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(s || "");
  return m ? `${m[1]}/${m[2]}/${m[3]}` : (s || "?");
}

function log(msg, level = "info") {
  $("logCard").hidden = false;
  const li = document.createElement("li");
  li.className = level;
  li.textContent = `${new Date().toLocaleTimeString()}  ${msg}`;
  $("log").prepend(li);
}

function text(s) { return document.createTextNode(s); }
function btn(label, fn) { const b = document.createElement("button"); b.textContent = label; b.onclick = fn; return b; }

function banner(nodes, cls = "") {
  const el = $("banner");
  if (!nodes) { el.hidden = true; return; }
  el.className = "banner " + cls;
  el.replaceChildren(...nodes);
  el.hidden = false;
}

function render() {
  const v = view;
  if (!v) return;
  $("ver").textContent = "v" + v.version;
  $("root").textContent = v.root || "—";
  $("voices").textContent = voices
    ? `最新 ${fmtDate(voices.date)} 版，${voices.clips} 段（${(voices.bytes / 1048576).toFixed(0)} MB）`
    : (voicesError || (v.source ? "檢查中…" : "這個版本沒有設定語音來源"));

  const st = $("state");
  st.className = "pill " + v.overall;
  st.textContent = STATE_TEXT[v.overall] || v.overall;
  const patched = v.pkgs.filter((p) => p.state.kind === "patched");
  $("stateDetail").textContent = patched.length
    ? `${patched.reduce((a, p) => a + p.redirected, 0)} 個資源已導向日文` + (v.applied_date ? `，${fmtDate(v.applied_date)} 版` : "")
    : "";

  const voicesNewer = voices && patched.length && v.applied_id !== voices.id;
  const ready = v.root && !v.root_error && voices && !busy;
  $("apply").disabled = !ready || v.game_running;
  $("apply").textContent = voicesNewer || ["reverted", "stale", "mixed"].includes(v.overall) ? "重新套用日文語音" : "套用日文語音";
  $("preview").disabled = !ready;
  $("restore").disabled = !v.root || busy || v.game_running || v.overall === "original";
  $("pickRoot").disabled = $("recheck").disabled = busy;

  const reapply = () => run(false);
  if (v.root_error) banner([text(v.root_error)], "err");
  else if (v.game_running) banner([text("遊戲正在執行中，請先關閉遊戲再套用或還原。"), btn("重新檢查", refresh)]);
  else if (v.overall === "reverted") banner([text("遊戲更新或修復後，語音已回到韓語。"), btn("重新套用", reapply)]);
  else if (v.overall === "stale") banner([text(`遊戲檔案在套用後更新過（${v.stale.length} 個 blob），需要重新套用。`), btn("重新套用", reapply)]);
  else if (voicesNewer) banner([text(`有新的日文語音（${fmtDate(voices.date)} 版）。`), btn("更新", reapply)]);
  else if (v.overall === "mixed") banner([text("無法確認已套用的語音版本。"), btn("重新套用", reapply)]);
  else if (appUpdate) banner([text(`有新版程式 v${appUpdate.version}。`), btn("更新並重新啟動", installAppUpdate)]);
  else banner(null);
}

async function refresh() {
  try { view = await invoke("get_view"); } catch (e) { log(String(e), "err"); }
  render();
}

async function checkVoices() {
  if (!view || !view.source) return;
  voicesError = null;
  try { voices = await invoke("check_voices"); } catch (e) { voices = null; voicesError = String(e); }
  render();
}

async function checkApp() {
  try { appUpdate = await invoke("check_app_update"); } catch (e) { appUpdate = null; }
  render();
}

function showProgress(on) { $("progressCard").hidden = !on; }

async function run(dryRun) {
  busy = true; showProgress(true); render();
  try {
    const r = await invoke("patch", { dryRun });
    log(dryRun ? `預覽：會替換 ${r.clips} 段語音（${r.bundles} 個 bundle）` : `完成：已替換 ${r.clips} 段語音`, "ok");
  } catch (e) {
    log(String(e), "err");
  }
  busy = false; showProgress(false);
  await refresh();
}

let restoreArmed = null;
async function restore() {
  // two-step confirm (window.confirm is not reliable inside the webview)
  if (!restoreArmed) {
    $("restore").textContent = "確定還原？再按一次";
    restoreArmed = setTimeout(() => { restoreArmed = null; $("restore").textContent = "還原原版語音"; }, 4000);
    return;
  }
  clearTimeout(restoreArmed);
  restoreArmed = null;
  $("restore").textContent = "還原原版語音";
  busy = true; render();
  try { await invoke("restore"); log("已還原為原版（韓語）語音", "ok"); } catch (e) { log(String(e), "err"); }
  busy = false;
  await refresh();
}

async function installAppUpdate() {
  busy = true; showProgress(true); render();
  try { await invoke("install_app_update"); } catch (e) { log(String(e), "err"); }
  busy = false; showProgress(false); render();
}

listen("progress", (e) => {
  const p = e.payload;
  $("stage").textContent = STAGES[p.stage] || p.stage;
  const kb = p.stage === "download" || p.stage === "app";
  $("count").textContent = p.total ? (kb ? `${(p.done / 1024).toFixed(1)} / ${(p.total / 1024).toFixed(1)} MB` : `${p.done} / ${p.total}`) : "";
  $("bar").max = p.total || 1;
  if (p.total) $("bar").value = p.done; else $("bar").removeAttribute("value");
});
listen("log", (e) => log(e.payload.msg, e.payload.level));

$("apply").onclick = () => run(false);
$("preview").onclick = () => run(true);
$("restore").onclick = restore;
$("recheck").onclick = () => { refresh(); checkVoices(); };
$("pickRoot").onclick = async () => {
  try { if (await invoke("pick_root")) refresh(); } catch (e) { log(String(e), "err"); }
};
window.addEventListener("focus", () => { if (!busy) refresh(); });

refresh().then(() => { checkVoices(); checkApp(); });
