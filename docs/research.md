# 技術筆記

遊戲檔案的格式，以及 mmvp 怎麼利用它們。使用方式見 [README](../README.md)。
除非標註「推論」，以下內容都經過實際解析或實機驗證。

## 遊戲檔案

### 用戶端

|    | 安裝位置 | Nexon Launcher 產品編號 | 語音 |
|----|---|---|---|
| TW | `…\MabinogiMobile_TW\appdata` | 59826 | 韓語 |
| JP | `…\MabinogiMobile_JP` | 2943 | 日語 |

- 兩版都是 Unity 2021.3.51f1（IL2CPP），目錄結構相同。
- TW 的安裝位置記在 `%APPDATA%\NexonLauncher\installed-apps.db`（JSON）的 `installedApps["59826"].installPath`。
- launcher 更新或修復時，會暫時刪掉 `index` 並重寫 blob。

### 封包

資源分在幾個 package 資料夾，每個都是一組 `NNNN.blob` 加一個 `index`：

```
AssetPackages/   AssetPackages/Starter/   ResourcesPackage/   ResourcesPackage/Starter/
```

語音都在 `AssetPackages` 與 `AssetPackages/Starter`。

**`.blob`**：多個 Unity asset bundle 串在一起。

| 欄位 | 內容 |
|---|---|
| `u32` | magic `0xE5A9C3BA` |
| `u32` | 標頭結束位置 |
| `u32[]` | 每個 slot 的結束位置（累加） |
| 其後 | 各 slot 的 UnityFS bundle（未加密） |

遊戲用 `{0:0000}.blob` 組檔名，所以新檔只能用數字命名。

**`index`**：資產與 bundle 的位置表。位置一律是 `(u16 blob, u16 slot)`。

| 區段 | 內容 |
|---|---|
| `u32` magic `0xE6A9C3BA`、`u32 N` | |
| 位置 × N | 每個資產所在的 bundle |
| `u32 M`，M 筆 {位置、`u32 C`、位置 × C} | 每個 bundle 相依的 bundle |
| 其餘 | 與位置無關的資料 |

- `blob` 的最高位元（bit 15）選 package：0 是 `AssetPackages`，1 是 `AssetPackages/Starter`。相依表會跨 package，例如 cutscene 依賴 Starter 裡的 shader。
- 遊戲用「從哪裡載入」去查相依表。只改資產表時，cutscene 會找不到 shader 而變成紫紅色。
- 資產表沒有引用的 slot，相依表也不會引用，那些是舊版本留下的死資料。

### 語音

- 語音是 Unity 的 AudioClip（Vorbis，FSB5 格式），音訊存在 bundle 的 `.resource` 串流裡。
- JP 和 TW 用相同的資產路徑（如 `…/sound/voice/npc/…`、`…/cutscene/…/voice/…_vo_NN.wav`），只是內容不同。
- 兩版同名的 bundle（`AssetBundle.m_Name`，如 `dedicated_bundle_7dbd4a02`）內容對應，但所在的 blob／slot 不同。
- 同一段語音常常嵌在多個 bundle 裡；少數路徑在 JP 的不同 bundle 裡有不同版本。
- cutscene 的語音和整段 cutscene（Camera、Material 等）放在同一個 bundle，換語音就得換整個 bundle。
- 規模（2026-11-27）：JP 有 7380 段不重複的語音（95.6 MB）；套到 TW 會動到 255 個 bundle、6745 段。

## mmvp 的做法

### 語音站台

`mmvp-dumper` 從 JP 用戶端產生一個靜態站台並部署。格式定義在 `src-tauri/shared/src/site.rs`。

| 檔案 | 內容 |
|---|---|
| `latest.json` | 日期、manifest 的 SHA1、語音數量與大小 |
| `manifest.json` | 每段語音的大小與 AudioClip 欄位；bundle 名稱＋路徑 → 語音；路徑 → 最常見的語音 |
| `clips/<sha1>` | JP 原始音訊，以內容 SHA1 命名 |

- manifest 只放內容，所以語音沒變時 SHA1 不變。這個 SHA1 就是「語音版本」，`npm run publish:voices` 靠它判斷要不要部署。
- 對應方式：先找 JP 同名 bundle 裡的同一路徑，找不到才用該路徑最常見的版本。

### 套用（`src-tauri/src/patcher/install.rs`）

1. **掃描**：讀原始 index（已套用時讀 `index.bak`），只解析被引用的 bundle，算出每段語音的 SHA1。
2. **下載**：和 JP 不同、本機又沒快取的語音，平行下載並驗 SHA1。
3. **重建**：只換掉語音串流，AudioClip 的欄位原地改寫（都是定長欄位，其他資料一個 byte 都不動），寫進新的 `9876.blob`。
4. **提交**：先把 index 恢復成原始內容（第一次套用時備份為 `index.bak`），讓已經沒有任何地方引用 `9876.blob` → 放上新的 `9876.blob` → 改寫兩個 index 的資產表與相依表。任何一步中斷，index 都只會指向存在的檔案。

mmvp 每個 package 只會新增 `9876.blob` 這一個 blob，原始的 blob 永遠不會被修改。

### 狀態判斷

全部從安裝目錄本身看出來，不另外存狀態：

- index 裡有指向 `9876.blob` 的位置：已套用。
- `9876.blob`：mmvp 產生的檔案；其他編號的 blob 一律視為遊戲原檔。
- `index.mmvp.json`：記錄套用的語音版本，以及當時原始 blob 的大小與修改時間。用來判斷遊戲更新後要不要重新套用。

### 自我更新

Release 只有 `mmvp.exe`、它的簽章 `mmvp.exe.sig`，以及 `latest.json`（版本號）。
程式發現新版時，下載新的 exe，用內建的公鑰驗證簽章，把執行中的 exe 改名成 `.old`、換上新檔後重新啟動。

## 已知限制

- launcher 的「修復」會把 `index` 換回原版，mod 就會失效，重新套用即可。
- 推論：反作弊（BlackCipher）或 launcher 是否會檢查新增的 blob，目前未知。實機使用至今沒有問題。

## 開發環境

- Rust：本機用 scoop 安裝的 rustup（GNU toolchain）加 `mingw`。GNU 版的 exe 執行時需要旁邊有 `WebView2Loader.dll`；正式版由 GitHub Actions 以 MSVC 建置，是單一 exe。
- Node.js：根目錄 `package.json` 提供 Tauri CLI、部署工具與常用指令（`npm run dev / test / dump / publish:voices`）。
- GitHub Actions 設定：secrets `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；variables `TAURI_UPDATER_PUBKEY`、`MMVP_SOURCE`。
- 部署語音：設定放在 `deploy.local.json`（不進 git）。
