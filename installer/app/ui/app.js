const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const STAGES = { scan: "掃描遊戲檔案", plan: "比對語音", rebuild: "重新打包", commit: "寫入", download: "下載語音包" };
const STATE_TEXT = {
  original: "韓語（原版）", patched: "日文語音已套用", reverted: "遊戲更新後已還原為韓語",
  missing: "index 不存在（launcher 更新中？）", unknown: "未知",
};

let view = null;
let busy = false;
let remote = null;

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

function setBusy(b) {
  busy = b;
  render();
}

function banner(html, cls = "") {
  const el = $("banner");
  if (!html) { el.hidden = true; return; }
  el.className = "banner " + cls;
  el.innerHTML = "";
  el.append(...html);
  el.hidden = false;
}

function text(s) { return document.createTextNode(s); }
function btn(label, fn) { const b = document.createElement("button"); b.textContent = label; b.onclick = fn; return b; }

function render() {
  const v = view;
  if (!v) return;
  $("ver").textContent = "v" + v.version;
  $("root").textContent = v.root || "—";
  $("pack").textContent = v.pack ? `${fmtDate(v.pack.created)} 版，${v.pack.clips} 段語音（${v.pack.id.slice(0, 10)}）` : (v.pack_error || "—");
  const st = $("state");
  st.className = "pill " + v.overall;
  st.textContent = STATE_TEXT[v.overall] || v.overall;
  const patched = v.pkgs.filter((p) => p.state === "patched");
  $("stateDetail").textContent = patched.length ? `${patched.reduce((a, p) => a + p.redirected, 0)} 個資源已導向日文` : "";

  const ready = v.root && !v.root_error && v.pack && !busy;
  $("apply").disabled = !ready || v.game_running;
  $("preview").disabled = !ready;
  $("restore").disabled = !v.root || busy || v.game_running || v.overall === "original";
  $("pickRoot").disabled = $("pickPack").disabled = busy;
  $("update").hidden = !v.update_url;
  $("update").disabled = busy;

  if (v.root_error) banner([text(v.root_error)], "err");
  else if (v.game_running) banner([text("遊戲正在執行中，請先關閉遊戲再套用或還原。"), btn("重新檢查", refresh)]);
  else if (v.overall === "reverted") banner([text("遊戲更新或修復後，語音已回到韓語。"), btn("重新套用", () => run(false))]);
  else if (v.overall === "patched" && v.pack && v.applied_pack && v.applied_pack !== v.pack.id)
    banner([text("目前的語音包比已套用的新。"), btn("重新套用", () => run(false))]);
  else if (remote && remote.pack_newer) banner([text(`有新的語音包（${fmtDate(remote.pack.created)} 版）。`), btn("下載", downloadPack)]);
  else if (remote && remote.app_newer) banner([text(`有新版程式 v${remote.app.version}。`), btn("前往下載", () => invoke("open_url", { url: remote.app.url }))]);
  else banner(null);
}

async function refresh() {
  try {
    view = await invoke("get_view");
  } catch (e) {
    log(String(e), "err");
  }
  render();
}

async function run(dryRun) {
  setBusy(true);
  $("progressCard").hidden = false;
  try {
    const r = await invoke("patch", { dryRun });
    log(dryRun ? `預覽：會替換 ${r.clips} 段語音（${r.bundles} 個 bundle）` : `完成：已替換 ${r.clips} 段語音`, "ok");
  } catch (e) {
    log(String(e), "err");
  }
  $("progressCard").hidden = true;
  setBusy(false);
  refresh();
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
  setBusy(true);
  try {
    await invoke("restore");
    log("已還原為韓語語音", "ok");
  } catch (e) {
    log(String(e), "err");
  }
  setBusy(false);
  refresh();
}

async function checkUpdate(quiet) {
  try {
    remote = await invoke("check_update");
    if (!quiet) log(remote.pack_newer || remote.app_newer ? "有可用的更新" : "已是最新版本", "ok");
  } catch (e) {
    if (!quiet) log(String(e), "err");
  }
  render();
}

async function downloadPack() {
  setBusy(true);
  $("progressCard").hidden = false;
  try {
    await invoke("download_pack", { pack: remote.pack });
    log("語音包下載完成", "ok");
    remote.pack_newer = false;
  } catch (e) {
    log(String(e), "err");
  }
  $("progressCard").hidden = true;
  setBusy(false);
  await refresh();
  if (view && view.overall === "patched") banner([text("新的語音包已下載。"), btn("重新套用", () => run(false))]);
}

listen("progress", (e) => {
  const p = e.payload;
  $("stage").textContent = STAGES[p.stage] || p.stage;
  $("count").textContent = p.total ? (p.stage === "download" ? `${(p.done / 1024).toFixed(1)} / ${(p.total / 1024).toFixed(1)} MB` : `${p.done} / ${p.total}`) : "";
  $("bar").max = p.total || 1;
  $("bar").value = p.total ? p.done : 0;
  if (!p.total) $("bar").removeAttribute("value");
});
listen("log", (e) => log(e.payload.msg, e.payload.level));

$("apply").onclick = () => run(false);
$("preview").onclick = () => run(true);
$("restore").onclick = restore;
$("update").onclick = () => checkUpdate(false);
$("pickRoot").onclick = async () => {
  try { if (await invoke("pick_root")) refresh(); } catch (e) { log(String(e), "err"); }
};
$("pickPack").onclick = async () => {
  try { if (await invoke("pick_pack")) refresh(); } catch (e) { log(String(e), "err"); }
};
window.addEventListener("focus", () => { if (!busy) refresh(); });

refresh().then(() => { if (view && view.update_url) checkUpdate(true); });
