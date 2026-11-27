const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const STEPS = { scan: "掃描遊戲檔案", download: "下載日文語音", rebuild: "重新打包", commit: "寫入", app: "下載新版程式" };
const STATES = { original: "韓語（原版）", patched: "日文語音已套用", reverted: "遊戲更新後已還原為韓語", missing: "遊戲正在更新" };

let view = null;      // local state from the backend
let voices = null;    // latest voice version on the site
let update = null;    // newer app version
let busy = false;

const date = (s) => (s || "").replaceAll("-", "/");
const mb = (n) => (n / 1048576).toFixed(n < 10485760 ? 1 : 0);

function log(msg, level = "info") {
  $("logCard").hidden = false;
  const li = document.createElement("li");
  li.className = level;
  li.textContent = `${new Date().toLocaleTimeString()}  ${msg}`;
  $("log").prepend(li);
}

function banner(text, action, label, cls = "") {
  const el = $("banner");
  el.hidden = !text;
  if (!text) return;
  el.className = "banner " + cls;
  el.replaceChildren(document.createTextNode(text));
  if (action && !busy) {
    const b = document.createElement("button");
    b.textContent = label;
    b.onclick = action;
    el.append(b);
  }
}

function render() {
  if (!view) return;
  const s = view.status;
  $("ver").textContent = "v" + view.version;
  $("root").textContent = view.root || "—";
  $("voices").textContent = voices ? `${date(voices.date)} 版，${voices.clips} 段（${mb(voices.bytes)} MB）` : "檢查中…";
  $("state").className = "pill " + (s ? s.state : "missing");
  $("state").textContent = s ? STATES[s.state] : "—";
  $("detail").textContent = s && s.state === "patched"
    ? `${s.redirected} 個資源已導向日文` + (s.voice ? `，${date(s.voice.date)} 版` : "")
    : "";

  const outdated = s && s.state === "patched" && voices && s.voice?.id !== voices.manifest_sha1;
  const reapply = s && (s.state === "reverted" || s.stale || outdated);
  $("apply").textContent = reapply ? "重新套用日文語音" : "套用日文語音";
  $("apply").disabled = busy || !s || !voices || view.game_running || s.state === "missing";
  $("restore").disabled = busy || !s || view.game_running || s.state === "original";
  $("pickRoot").disabled = busy;

  const run = () => apply();
  if (view.error) banner(view.error, null, null, "err");
  else if (view.game_running) banner("遊戲正在執行，請先關閉遊戲。", refresh, "重新檢查");
  else if (s.state === "reverted") banner("遊戲更新後語音回到韓語。", run, "重新套用");
  else if (s.stale) banner("遊戲檔案在套用後有更新，需要重新套用。", run, "重新套用");
  else if (outdated) banner(s.voice ? `有新的日文語音（${date(voices.date)} 版）。` : "無法確認已套用的語音版本。", run, s.voice ? "更新" : "重新套用");
  else if (update) banner(`有新版程式 v${update}。`, installUpdate, "更新並重新啟動");
  else banner(null);
}

async function refresh() {
  view = await invoke("state");
  render();
}

async function task(fn) {
  if (busy) return;
  busy = true;
  $("progress").hidden = false;
  render();
  try {
    await fn();
  } catch (e) {
    log(String(e), "err");
  }
  busy = false;
  $("progress").hidden = true;
  await refresh();
}

const apply = () => task(async () => {
  const { bundles, clips } = await invoke("apply");
  log(`完成：${clips} 段日文語音，${bundles} 個 bundle`, "ok");
});

// two-step confirm: window.confirm is unreliable in the webview
const RESTORE = $("restore").textContent;
let armed = null;
function disarm() {
  clearTimeout(armed);
  armed = null;
  $("restore").textContent = RESTORE;
}
function restore() {
  if (!armed) {
    $("restore").textContent = "確定還原？再按一次";
    armed = setTimeout(disarm, 4000);
    return;
  }
  disarm();
  task(async () => { await invoke("restore"); log("已還原為原版（韓語）語音", "ok"); });
}

const installUpdate = () => task(() => invoke("install_update"));

listen("progress", ({ payload: p }) => {
  $("step").textContent = STEPS[p.step] || p.step;
  const bytes = p.step === "download" || p.step === "app";
  $("count").textContent = p.total ? (bytes ? `${mb(p.done)} / ${mb(p.total)} MB` : `${p.done} / ${p.total}`) : "";
  if (p.total) { $("bar").max = p.total; $("bar").value = p.done; } else $("bar").removeAttribute("value");
});

$("apply").onclick = apply;
$("restore").onclick = restore;
$("pickRoot").onclick = async () => { try { if (await invoke("pick_root")) await refresh(); } catch (e) { log(String(e), "err"); } };
window.addEventListener("focus", () => { if (!busy) refresh(); });

refresh().then(async () => {
  try { voices = await invoke("voices"); } catch (e) { log(String(e), "err"); }
  try { update = await invoke("app_update"); } catch { /* no update source in local builds */ }
  render();
});
