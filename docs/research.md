# 技術筆記

遊戲檔案的格式，以及 mmvp 怎麼利用它們。使用方式見 [README](../README_zh-tw.md)。
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
- 只看 index 引用的 slot，每個語音路徑在 JP、TW 都只屬於一個 bundle。blob 裡沒被引用的舊 slot 常有同名 bundle 的舊版本（JP 有 75 個 slot、1472 段），不能拿來對應。
- cutscene 的語音和整段 cutscene（Camera、Material 等）放在同一個 bundle，換語音就得換整個 bundle。
- 語音的判斷（`site::is_voice`）：路徑含 `/voice/`、`_vo_NN`、`_voN`（如 `…/naozone/sfx/…_naozwhite_vo1.wav`），或 `_vox`（怪物、寵物的發聲）。
- JP 沒有錄音的地方放的是佔位檔：288 bytes、1.25 秒（231 B/s），共 63 段。真的語音最低也有 1.8 kB/s，所以低於 1000 B/s（`site::MIN_BYTES_PER_SEC`）的 clip 不發布，TW 保留原音。
- 規模（2026-11-28）：JP 有 7732 段不重複的語音（112.5 MB），7736 個路徑；套到 TW 會動到 261 個 bundle、6807 段。

## mmvp 的做法

### 語音站台

`mmvp-dumper` 從 JP 用戶端產生一個靜態站台並部署（日期由 `--date` 指定，`npm run publish:voices` 會傳入當天的本地日期）。格式定義在 `src-tauri/shared/src/site.rs`。

| 檔案 | 內容 |
|---|---|
| `latest.json` | 格式版本、日期、manifest 的 SHA1、語音數量與大小 |
| `manifest.json` | 每段語音的大小與 AudioClip 欄位；bundle 名稱＋路徑 → 語音；路徑 → 最常見的語音 |
| `packs.json` | 所屬的 manifest SHA1；每個 pack 的 SHA1、bundle 名稱、大小，以及裡面每段語音的 SHA1 和 offset |
| `packs/<sha1>` | 一個 JP bundle 的語音串接在一起，以內容 SHA1 命名 |
| `clips/<sha1>` | JP 原始音訊，以內容 SHA1 命名；只給 v0.2 以前的程式用 |

- 每段語音只放進一個 pack，也就是第一個（依名稱排序）用到它的 bundle 的 pack。2026-11-28 共有 442 個 pack，都在 Cloudflare Pages 單檔 25 MiB 的上限內，dumper 會檢查。
- 為什麼要用 pack：每個請求的時間幾乎都是延遲。每個請求約 80–110 ms，資料本身只要 1–4 ms，所以 7732 個小檔就算 16 個平行下載也要約 40 秒。改成一個 bundle 一個檔之後，請求數少了 17 倍。HTTP/2、HTTP/3 在同樣並行數下沒有比較快（實測）。
- v0.3 起的 patcher 只下載 pack，驗證 pack 和裡面每段語音的 SHA1，再拆成逐段的快取。packs.json 缺少或和 latest.json 對不上時直接報錯，不會改抓 `clips/`。

- manifest 只放內容，所以語音沒變時 SHA1 不變。這個 SHA1 就是「語音版本」，`npm run publish:voices` 靠它判斷要不要部署。
- 只讀 JP index 引用的 slot，和套用時讀 TW 的方式相同。
- 對應方式：先找 JP 同名 bundle 裡的同一路徑，找不到才用該路徑最常見的版本。

### 套用（`src-tauri/src/patcher/install.rs`）

1. **掃描**：讀原始 index（已套用時讀 `index.bak`），只解析被引用的 bundle，算出每段語音的 SHA1。
2. **下載**：和 JP 不同、本機又沒快取的語音，平行下載並驗 SHA1。
3. **重建**：只換掉語音串流，AudioClip 的欄位原地改寫（都是定長欄位，其他資料一個 byte 都不動），直接寫進新的 blob（`9876.blob`，或 index 正在使用 9876 時改用 `9877.blob`）。index 還沒指向它，所以寫到一半中斷也不影響遊戲。
4. **提交**：備份原始 index 為 `index.bak` → 改寫兩個 index 的資產表與相依表 → 刪掉舊 blob。任何一步中斷，index 都只會指向存在的檔案。

重新套用時，新 blob 會在 9876 和 9877 之間輪替，不會覆蓋正在使用的檔案。原始的 blob 永遠不會被修改。

### 狀態判斷

全部從安裝目錄本身看出來，不另外存狀態：

- index 裡有指向 `9876.blob` 或 `9877.blob` 的位置：已套用。
- `9876.blob`、`9877.blob`：mmvp 產生的檔案；其他編號的 blob 一律視為遊戲原檔。
- `index.mmvp.json`：記錄套用的語音版本，用來判斷語音更新後要不要重新套用。遊戲更新會重寫 index，那時 index 不再指向 mmvp 的 blob，就會提示重新套用。

### 自我更新

Release 只有 `mmvp.exe`、它的簽章 `mmvp.exe.sig`，以及 `latest.json`（版本號）。
程式發現新版時，下載最新 release 的 exe，用內建的公鑰驗證簽章，把執行中的 exe 改名成 `.old`、換上新檔後重新啟動。

## 已知限制

- 只有台版經過驗證。手動選擇的目錄只要有 `AssetPackages\index` 就會被接受，不是 Nexon Launcher 登記的台版時介面標示「實驗性」。推論：其他地區的版本若沿用相同的封包格式、bundle 名稱與資產路徑就能套用；遊戲執行檔若不叫 `MabinogiMobile.exe`，「遊戲執行中」的偵測會失效。
- launcher 的「修復」會把 `index` 換回原版，mod 就會失效，重新套用即可。
- 推論：反作弊（BlackCipher）或 launcher 是否會檢查新增的 blob，目前未知。實機使用至今沒有問題。

## 開發環境

- Rust：本機用 scoop 安裝的 rustup（GNU toolchain）加 `mingw`。GNU 版的 exe 執行時需要旁邊有 `WebView2Loader.dll`；正式版由 GitHub Actions 以 MSVC 建置，是單一 exe。
- Node.js：根目錄 `package.json` 提供 Tauri CLI、部署工具與常用指令（`npm run dev / test / dump / publish:voices`）。
- GitHub Actions 設定：secrets `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；variables `TAURI_UPDATER_PUBKEY`、`MMVP_SOURCE`。
- 部署語音：設定放在 `deploy.local.json`（不進 git），三個欄位都必填：`project`（Pages 專案）、`url`（站台完整網址）、`jp_root`（日版用戶端目錄）。
