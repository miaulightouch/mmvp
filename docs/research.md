# 格式與設計筆記

使用方式見 [README](../README.md)。**[確認]** = 實際讀檔／解析驗證過；**[推論]** = 依旁證推測，未驗證。
早期的 Python／PowerShell 研究腳本已移除，需要時看 git 歷史。

## 用戶端

| | 根目錄 | Nexon Launcher 產品 | 備註 |
|---|---|---|---|
| TW | `…\MabinogiMobile_TW\appdata` | 59826（`59826-obd-manifest`） | 內建**韓語**配音 |
| JP | `…\MabinogiMobile_JP` | 2943（`obd-manifest-2943`） | 啟動需日本 IP |

- [確認] 兩版都是 Unity 2021.3.51f1、IL2CPP，目錄結構相同。
- [確認] TW 由 Nexon Launcher 安裝，路徑記在 `%APPDATA%\NexonLauncher\installed-apps.db`（JSON）的 `installedApps["59826"].installPath`；Windows 解除安裝登錄檔沒有 TW Mobile 的項目。
- [確認] JP／TW launcher 更新或修復時會暫時刪除 `index` 並重寫 blob。

## 資源封包

`AssetPackages`、`AssetPackages/Starter`、`ResourcesPackage`、`ResourcesPackage/Starter`、`PlatformResourcePackages`，每個是 `NNNN.blob` + `index`。語音都在 AssetPackages 家族。

### `.blob`

```
u32 magic = 0xE5A9C3BA, u32 header end, u32[] 每個 slot 的累加結束 offset
```

每個 slot 是一個未加密的 UnityFS bundle（原檔 flags 0x243：LZ4HC、block info 在前並 padding）。少數 slot 不是 UnityFS（開頭 `1F 8B 11 E5`），與音訊無關。
[確認] 遊戲用 `"{0:0000}.blob"` 組檔名，新 blob 只能是數字（mmvp 用 9876／9877）。

### `index`

```
u32 magic = 0xE6A9C3BA, u32 N
(u16 blob, u16 slot)[N]                                              資產條目 → bundle 位置
u32 M, M × { (u16 blob, u16 slot) key, u32 C, (u16 blob, u16 slot)[C] }   bundle → 相依 bundle 的位置
u32 N, 16-byte[N], u32[] …                                           其他資料（不含位置）
```

- [確認] `blob` 的 bit 15 選 package：0 = 家族的主 package，1 = Starter。`*/Starter` 主表每條都帶這個 bit；相依表會跨 package（例如 AssetPackages 的 cutscene 依賴 Starter 的 shader）。Starter index 從未出現指向主 package 的位置。
- [確認] **遊戲用載入位置查相依表**：只改主表時，cutscene 從 9876 載入後找不到相依，shader／material 沒載入，畫面變紫紅色（語音正常）。所以主表與相依表裡指向被重導 bundle 的位置都要改。
- [確認] 主表沒引用的 slot（AssetPackages 約 14.6k 個，其中 160 個含音訊）也不在相依表裡，是增量更新留下的死資料。

## 語音

- [確認] 音訊是 Unity 原生 AudioClip（Vorbis／FSB5，44.1 kHz，Streaming），資料在 bundle 的 `.resource` 串流。
- [確認] JP 與 TW 用相同的 container path（`assets/patchableassets/sound/voice/npc/…`、`…/cutscene/…/voice/…_vo_NN.wav`），內容不同；路徑沒有語言後綴。
- [確認] 兩版同一個 bundle 的 `AssetBundle.m_Name`（如 `dedicated_bundle_7dbd4a02`）相同，但所在 blob／slot 不同。同一段語音常嵌在多個 bundle，少數路徑在 JP 不同 bundle 裡有不同版本，所以對應以「bundle 名稱＋路徑」為主、路徑最常見版本為後備。
- [確認] cutscene 的語音嵌在整個 cutscene prefab 的 bundle 裡（含 Camera、PlayableDirector、Material…），換語音就是換整個 bundle。
- 規模（2026-11-27）：JP 7297 個語音路徑、7380 段不重複內容、95.6 MB；套到 TW 是 57 blob／255 bundle／6745 段。

## 語音站台（`src-tauri/dumper`，格式定義在 `src-tauri/shared/src/site.rs`）

```
latest.json     {"version": 3, "date", "manifest", "manifest_sha1", "clips", "bytes"}
manifest.json   {"version": 3, "filter", "clips": {sha1: {"size", "fields"}}, "bundles": {名稱: {路徑: sha1}}, "default": {路徑: sha1}}
clips/<sha1>    JP 原始 FSB5
_headers, index.html
```

- manifest 只含內容，語音沒變時 SHA1 不變；它就是語音版本 id，`npm run publish:voices`（`scripts/publish-voices.mjs`）用它判斷要不要部署。[確認] Rust 版 dumper 產出的 manifest 與原本 Python 版 byte 相同（SHA1 `04d6034f…`），dump 26 秒（Python 129–191 秒）。
- Cloudflare Pages 單次部署上限 20000 個檔案；設定與 token 只在本機（`deploy.local.json`、`wrangler login`）。

## 套用（`src-tauri/src/install.rs`）

1. 讀各 package 的原始 index（已套用時用 `index.bak`），只掃主表有引用的 slot，平行解析出 bundle 名稱與每段語音的 SHA1。
2. 和 manifest 比對，下載不同且快取沒有的 JP 語音（16 條並行、共用連線池、逐檔驗 SHA1）。
3. 平行重建：只重建有換到的 resource 串流，AudioClip 欄位原地覆寫（[確認] 全部是定長欄位，其他 bytes 不變）；輸出 LZ4、128 KiB 分塊、block info 在尾端（flags 0xC2，已實機驗證）。重建好的 bundle 直接 append 進 `<pkg>/NNNN.blob.mmvp.tmp`。
4. 提交：`index.bak`（只從原始 index 寫）→ 所有新 blob 就位 → 寫所有 index（同家族的 index 會互指）→ `index.mmvp.json` → 刪舊 blob。中斷時 index 一定指向存在的檔案。
5. 新 blob 編號取 live index 沒在用的 9876／9877，所以重套不會覆蓋正在被引用的檔案。

狀態只看安裝目錄：index 有指向 ≥5000 的位置 = 已套用；≥5000 的 blob 都是 mmvp 的；`index.mmvp.json` 記錄語音版本與被導向的原始 blob（size＋mtime），用來判斷遊戲更新後要不要重套。

## 驗證紀錄

- 掃描：Rust 與 UnityPy 對 TW 的 2084 個音訊 bundle／15179 段 clip 結果完全一致。
- 重建：255 個 bundle 與 UnityPy 版逐物件相同（音訊以外 bytes 相同）；v0.2.0 重構後的輸出與 v0.1.3（實機確認無紫畫面）比對，兩張表的每個位置都指向 byte 相同的 bundle。
- 實機（2026-11-27）：NPC 與 cutscene 語音為 JP、cutscene 畫面正常；自我更新（安裝版與 portable）以測試金鑰端到端驗證。
- 未知：BlackCipher 或 launcher 修復是否會檢查 blob 以外的檔案（launcher 的 obd-manifest 記錄每個原始檔的分塊 SHA1；mmvp 不改原始檔）。

## 建置

- 本機：scoop 的 rustup（`stable-x86_64-pc-windows-gnu`）＋ `mingw`（dlltool）。GNU 版的 exe 需要旁邊有 `WebView2Loader.dll`；release 由 GitHub Actions 以 MSVC 建置，是單一 exe。
- Node.js：根目錄 `package.json` 提供 Tauri CLI、wrangler 與常用指令（`npm run dev / test / dump / publish:voices`）。
- 發版設定（GitHub Actions）：secrets `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；variables `TAURI_UPDATER_PUBKEY`、`MMVP_SOURCE`。
