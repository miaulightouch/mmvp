# MMVP - 瑪奇 Mobile 語音包

[English](README.md) | **繁體中文**

Mabinogi Mobile Voice Pack (for TW)：把台版《瑪奇 Mobile》的韓語配音換成日版配音。

- 只替換語音，不修改任何原始遊戲檔案，隨時可以還原
- 首次下載約 100 MB，之後只下載有變動的語音
- 遊戲更新或語音更新時會提示重新套用，程式本身會自動更新
- 介面依系統語言顯示繁體中文或英文，右上角的「A/文」可以手動切換

## 安裝

1. 從 [Releases](https://github.com/miaulightouch/mmvp/releases/latest) 下載 `mmvp.exe`，放在可寫入的資料夾直接執行（免安裝）。
2. 關閉遊戲，確認遊戲目錄後按「套用日文語音」。
3. 要恢復原版，按「還原原版語音」。

### 其他地區的版本（實驗性）

mmvp 會自動找到 Nexon Launcher 安裝的台版。其他地區的版本只要檔案架構相同（遊戲目錄下有 `AssetPackages\index`），也可以按「變更…」手動選擇遊戲目錄來套用，介面會標示「實驗性」。
目前只有台版經過實機驗證，其他版本能否正常運作未知。

## 運作方式

遊戲資源是一組 `NNNN.blob`（內含 Unity asset bundle）加上 `index`（資產與相依 bundle 的位置表）。
mmvp 找出含有語音的 bundle，換上日版語音後重新打包成 `9876.blob`（重新套用時與 `9877.blob` 輪替），再把 `index` 中所有指向這些 bundle 的位置改到新檔。
原始 `index` 備份為 `index.bak`。

## 手動還原

1. 將 AssetPackages 與 AssetPackages\Starter 中的 index.bak 改回 index
2. 將 AssetPackages 與 AssetPackages\Starter 中的 9876.blob（或 9877.blob）與 index.mmvp.json 移除

## 開發

標準 Tauri 2 專案：

```
src/                  GUI 前端（純 HTML/JS）
src-tauri/            Rust workspace root 與 mmvp app（lib.rs：Tauri 指令，patcher/：套用邏輯，net.rs：HTTP）
src-tauri/shared/     mmvp-shared：blob／index、UnityFS、SerializedFile、AudioClip、語音站台格式
src-tauri/dumper/     mmvp-dumper：從日版 dump 語音站台的額外工具
scripts/              publish-voices.mjs：dump 並在有變動時部署
docs/                 技術筆記
```

```sh
npm ci
npm run dev                  # 開發模式啟動 GUI
npm test                     # cargo test；設 MMVP_TEST_ROOT / MMVP_TEST_SOURCE 可跑端到端測試
npm run dump -- <jp_root> --date YYYY-MM-DD   # 只 dump 語音站台到 work/pages/<日期>
npm run publish:voices       # dump → 和線上比對 → 有變動才部署（讀 deploy.local.json，可加 -- --dry-run／--force）
```

commit 訊息與 release note 都遵守 [Conventional Commits](https://www.conventionalcommits.org/)（`feat:`、`fix:`、`refactor:`、`docs:`、`ci:` 等；破壞性變更加 `!` 或 `BREAKING CHANGE:` footer）。

發佈程式（版本寫在 `src-tauri/Cargo.toml`）：

1. 在 GitHub 為 tag `vX.Y.Z` 建立 draft release，寫好依 commit 類型分組的 release note。
2. `git tag vX.Y.Z && git push origin vX.Y.Z`。GitHub Actions 會建置 `mmvp.exe`、上傳到該 draft 並發佈。

技術細節見 [docs/research.md](docs/research.md)。

## 注意

非官方工具，與 NEXON、devCAT 無關。使用風險自負。
