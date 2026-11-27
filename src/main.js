// Texts: T and LANG from i18n.js.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

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
  $("experimental").hidden = !view.experimental;
  $("voices").textContent = voices ? T.voicesInfo(date(voices.date), voices.clips, mb(voices.bytes)) : T.checking;
  $("state").className = "pill " + (s ? s.state : "missing");
  $("state").textContent = s ? T.states[s.state] : "—";
  $("detail").textContent = s && s.state === "patched"
    ? T.detail(s.redirected) + (s.voice ? T.detailDate(date(s.voice.date)) : "")
    : "";

  const outdated = s && s.state === "patched" && voices && s.voice?.id !== voices.manifest_sha1;
  const reapply = s && (s.state === "reverted" || outdated);
  $("apply").textContent = reapply ? T.reapply : T.apply;
  $("apply").disabled = busy || !s || !voices || view.game_running || s.state === "missing";
  $("restore").disabled = busy || !s || view.game_running || s.state === "original";
  $("pickRoot").disabled = busy;
  $("lang").disabled = busy; // switching reloads the page

  const run = () => apply();
  if (view.error) banner(view.error, null, null, "err");
  else if (view.game_running) banner(T.gameRunning, refresh, T.recheck);
  else if (s.state === "reverted") banner(T.reverted, run, T.reapplyShort);
  else if (outdated && s.voice) banner(T.newVoices(date(voices.date)), run, T.update);
  else if (outdated) banner(T.unknownVoice, run, T.reapplyShort);
  else if (update) banner(T.newApp(update), installUpdate, T.updateRestart);
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
  log(T.done(clips, bundles), "ok");
});

// two-step confirm: window.confirm is unreliable in the webview
let armed = null;
function disarm() {
  clearTimeout(armed);
  armed = null;
  $("restore").textContent = T.restore;
}
function restore() {
  if (!armed) {
    $("restore").textContent = T.restoreConfirm;
    armed = setTimeout(disarm, 4000);
    return;
  }
  disarm();
  task(async () => { await invoke("restore"); log(T.restored, "ok"); });
}

const installUpdate = () => task(() => invoke("install_update"));

listen("progress", ({ payload: p }) => {
  $("step").textContent = T.steps[p.step] || p.step;
  const bytes = p.step === "download" || p.step === "app";
  $("count").textContent = p.total ? (bytes ? `${mb(p.done)} / ${mb(p.total)} MB` : `${p.done} / ${p.total}`) : "";
  if (p.total) { $("bar").max = p.total; $("bar").value = p.done; } else $("bar").removeAttribute("value");
});

$("apply").onclick = apply;
$("restore").onclick = restore;
$("pickRoot").onclick = async () => { try { if (await invoke("pick_root")) await refresh(); } catch (e) { log(String(e), "err"); } };
window.addEventListener("focus", () => { if (!busy) refresh(); });

// the backend needs the language before it produces any message
invoke("set_lang", { zh: LANG === "zh" }).then(refresh).then(async () => {
  try { voices = await invoke("voices"); } catch (e) { log(String(e), "err"); }
  try { update = await invoke("app_update"); } catch { /* no update source in local builds */ }
  render();
});
