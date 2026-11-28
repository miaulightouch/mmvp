// GUI texts in English and Traditional Chinese. The language follows the system (any Chinese locale
// gets Traditional Chinese, everything else English) until the language button picks one; the backend is
// told through `set_lang`.
// Functions take the values to fill in. `mb` is megabytes, already formatted.
const TEXTS = {
  en: {
    language: "English",
    title: "Mabinogi Mobile JP Voice",
    root: "Game folder",
    online: "Online version",
    local: "Local version",
    state: "Status",
    change: "Change…",
    experimental: "Experimental",
    experimentalTip: "Not the TW install recorded by the Nexon Launcher. Clients of other regions with the same file layout can be patched, but are unverified.",
    apply: "Apply JP voices",
    reapply: "Re-apply JP voices",
    restore: "Restore original voices",
    restoreConfirm: "Restore? Press again",
    checking: "Checking…",
    none: "None",
    unknown: "Unknown",
    steps: { scan: "Scanning game files", download: "Downloading JP voices", rebuild: "Repacking", commit: "Writing", app: "Downloading the new version" },
    states: { original: "Not applied", patched: "Applied", reverted: "Reverted", missing: "Game updating" },
    gameRunning: "The game is running. Close it first.",
    recheck: "Check again",
    reverted: "A game update put the original voices back.",
    reapplyShort: "Re-apply",
    newVoices: (date) => `New JP voices are available (${date}).`,
    unknownVoice: "The applied voice version is unknown.",
    update: "Update",
    newApp: (v) => `Version ${v} is available.`,
    updateRestart: "Update and restart",
    done: (clips, bundles) => `Done: ${clips} JP clips in ${bundles} bundles`,
    restored: "Original voices restored",
  },
  zh: {
    language: "繁體中文",
    title: "瑪奇 Mobile 日文語音",
    root: "遊戲目錄",
    online: "線上版本",
    local: "本地版本",
    state: "套用狀態",
    change: "變更…",
    experimental: "實驗性",
    experimentalTip: "不是 Nexon Launcher 登記的台版安裝。其他地區的版本只要檔案架構相同就能套用，但未經驗證。",
    apply: "套用日文語音",
    reapply: "重新套用日文語音",
    restore: "還原原版語音",
    restoreConfirm: "確定還原？再按一次",
    checking: "檢查中…",
    none: "無",
    unknown: "未知",
    steps: { scan: "掃描遊戲檔案", download: "下載日文語音", rebuild: "重新打包", commit: "寫入", app: "下載新版程式" },
    states: { original: "未套用", patched: "已套用", reverted: "已被還原", missing: "遊戲更新中" },
    gameRunning: "遊戲正在執行，請先關閉遊戲。",
    recheck: "重新檢查",
    reverted: "遊戲更新後語音回到原版。",
    reapplyShort: "重新套用",
    newVoices: (date) => `有新的日文語音（${date} 版）。`,
    unknownVoice: "無法確認已套用的語音版本。",
    update: "更新",
    newApp: (v) => `有新版程式 v${v}。`,
    updateRestart: "更新並重新啟動",
    done: (clips, bundles) => `完成：${clips} 段日文語音，${bundles} 個 bundle`,
    restored: "已還原為原版語音",
  },
};

// the language button's choice if there is one, else the system language
const LANG = localStorage.getItem("lang") ?? (navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en");
const T = TEXTS[LANG];
const OTHER = LANG === "zh" ? "en" : "zh";

// static texts: <… data-i18n="key"> gets its text, <… data-i18n-title="key"> its tooltip
document.documentElement.lang = LANG === "zh" ? "zh-Hant" : "en";
document.title = T.title;
for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = T[el.dataset.i18n];
for (const el of document.querySelectorAll("[data-i18n-title]")) el.title = T[el.dataset.i18nTitle];

// language button: switch to the other language. Texts are applied at load, so the page reloads.
document.getElementById("lang").title = TEXTS[OTHER].language;
document.getElementById("lang").ariaLabel = TEXTS[OTHER].language;
document.getElementById("lang").onclick = () => {
  localStorage.setItem("lang", OTHER);
  location.reload();
};
