# mabimobile-jp-voice-tw

把《Mabinogi Mobile》日版（JP）的語音移植到台版（TW）用戶端的工作區。

> 標註規則：**[確認]** = 實際讀檔／解析驗證過；**[推論]** = 依名稱或旁證推測，尚未驗證。

## 參考安裝（唯讀，不要修改）

| 版本 | 根目錄 | `app.info` | 備註 |
|---|---|---|---|
| TW | `D:\MabinogiMobile_TW\appdata` | `devCAT` / `瑪奇 Mobile` | |
| JP | `D:\Nexon\MabinogiMobile_JP` | `devCAT` / `マビモバ` | 啟動需日本 IP（目前用 tailscale exit node） |

- [確認] 兩版都是 Unity **2021.3.51f1**、IL2CPP（`GameAssembly.dll` + `global-metadata.dat`），目錄結構完全相同。
- [確認] TW 的 persistentDataPath 是 `%USERPROFILE%\AppData\LocalLow\devCAT\瑪奇 Mobile`，裡面只有影片、字型快取、WebView 快取，**沒有音訊**。

## 資源封包格式

資源都在根目錄下這幾個 package 資料夾，每個資料夾是 `NNNN.blob` + 一個 `index`：

```
AssetPackages/            ~760 blobs, ~12.8 GB   ← 語音在這裡
AssetPackages/Starter/
ResourcesPackage/
ResourcesPackage/Starter/
PlatformResourcePackages/
```

### `.blob`（[確認]，見 `tools/blob.py`）

```
0x00  u32  magic   = BA C3 A9 E5
0x04  u32  hdr_end = 第一個 bundle 的起始 offset
0x08  u32[(hdr_end-8)/4]  每個 bundle 的「結束」offset（累加，最後一個 = 檔案大小）
```

每個區段 `[prev_end, end)` 就是一個**未加密的標準 UnityFS asset bundle**，UnityPy 可直接讀。

- [確認] 少數區段不是 UnityFS（開頭 `1F 8B 11 E5 …`，像被改過的 gzip），體積很小，目前看不是音訊。尚未解。

### `index`（部分解析）

```
0x00  u32  magic = BA C3 A9 E6
0x04  u32  N     (TW AssetPackages: 238302)
0x08  (u16 blob_no, u16 bundle_no)[N]
之後  壓縮過的路徑字串表（類 LZ，可見片段如 "_Facial", "PickUp", 結尾是副檔名表 .wav/.png/.prefab…）
```

- [確認] `blob_no` 最大值 765 與 blob 數量吻合。
- [確認] `AssetPackages/Starter` 與 `ResourcesPackage/Starter` 的 index **每一條** blob 欄位都帶 `0x8000` 旗標（`0x8000 | NNNN`），其他 package 都沒有。[推論] 意義未知（可能表示位於 Starter）。patch 比對時遮掉這個 bit，改寫時保留。
- [確認] index 沒有指到所有 slot：TW `AssetPackages` 178444 個 slot 中有 14633 個沒有任何條目指向（其中 160 個含音訊，裡面的語音路徑都另有被引用的副本）。[推論] 是增量更新留下的舊副本，遊戲讀不到，patch 會跳過。
- [推論] 這是「資產路徑 → (blob, bundle)」的查找表；替換語音時若 bundle 大小或位置改變，可能需要同步改它。字串表的壓縮格式尚未解。

## 語音資源

- [確認] 沒有 CRI ADX2 / Wwise / FMOD bank；音訊是 **Unity 原生 AudioClip**，存在 bundle 的 `.resS` 裡。
  - 壓縮格式 `m_CompressionFormat = 1`（Vorbis，FSB5 容器），44.1 kHz，`m_LoadType = 2`（Streaming）。
- [推論] 播放端是 DarkTonic Master Audio（metadata 中有 `BusVoice`、`SoundGroup`、`VoicesForGroup` 等名稱）。
- [確認] 語音路徑：

  ```
  assets/patchableassets/sound/voice/npc/<地區>/...          NPC 語音（主體）
  assets/patchableassets/sound/voice/zamanchu/...             
  assets/patchableassets/sound/cutscene/<main|dungeon|...>/.../voice/<name>_vo_NN.wav   過場語音
  assets/patchableassets/sound/audioclips/{player,monster}/... 戰鬥音效／喊聲
  ```

- [確認] **JP 與 TW 用完全相同的 container path，但內容不同**（例：`q_main_g1_c1_0220_30_impairedglas_greymaze_2_vo_06.wav` JP 9.49s / TW 7.67s）。路徑裡沒有語言後綴，每個用戶端只帶一種語言的語音。
- [確認] 同一個語音在兩版落在**不同的 blob/bundle**（上例 JP `AssetPackages/0005.blob#10`，TW `AssetPackages/0001.blob#2`），不能整個 blob 檔對換，要以 container path 對應。
- [確認] 同一個 clip 常被**嵌在多個 bundle**（TW 有 1151 個路徑重複出現），替換時每份都要換。

### 全量掃描結果（2026-11-26，`tools/scan_audio.py` + `tools/compare.py`）

| | JP | TW |
|---|---|---|
| AudioClip（含重複） | 16148 | 15179 |
| 不重複 container path | 14006 | 14005 |

- 共同路徑 14005；其中 **6858 個內容不同**（= 移植候選）；TW 獨有 0，JP 獨有 1。
- 內容不同的主要分類：`voice/npc/*`（emainmacha 1362、story 987、dunbarton 984、tirchonaill 843、extra 625、colhen 594、bangor 429…）、`cutscene/main/g1` 380、`voice/zamanchu` 62，另有少量 monster 音效（succubus 69、highpriest 20、banshee 17…）。
- [確認] `audioclips/player/*`（玩家角色戰鬥聲）兩版大小相同 → [推論] 兩版共用同一份，不需移植。
- 要改的 TW 範圍：**7362 個 clip 實例，分布在 308 個 bundle / 73 個 blob**（`work/port_map.tsv`）。

### 待確認

- ~~TW 語音是什麼語言~~ [確認] TW 內建的是**韓語**語音（使用者確認，2026-11-27）。
- monster／UI／bgm 中「大小不同」的少數項目，是在地化差異還是單純版本差異（JP 剛更新過）。
- 改檔後的完整性檢查：BlackCipher（反作弊）與 launcher 的「修復」是否會偵測／覆蓋修改過的 blob。[推論] 很可能會，需實測。

## 參考實作：`tools/mmvoice.py`（Python dumper + patcher，`.mmvpack`）

```powershell
# 1. 從 JP 抽出所有語音成一個 pack（預設只抓路徑含 /voice/ 或 _vo_ 的 clip）
.\.venv\Scripts\python.exe tools\mmvoice.py dump D:\Nexon\MabinogiMobile_JP work\jp_voice.mmvpack
# 2. 套到 TW（先關遊戲）；--dry-run 只列出要改什麼
.\.venv\Scripts\python.exe tools\mmvoice.py patch D:\MabinogiMobile_TW\appdata work\jp_voice.mmvpack --data work\mmvoice_data
# 3. 查狀態／還原
.\.venv\Scripts\python.exe tools\mmvoice.py status  D:\MabinogiMobile_TW\appdata --data work\mmvoice_data
.\.venv\Scripts\python.exe tools\mmvoice.py restore D:\MabinogiMobile_TW\appdata --data work\mmvoice_data
```

- pack（`.mmvpack`）= 未壓縮 ZIP：`manifest.json` + `clips/<sha1>`（JP 原始 FSB5 bytes），格式見下方 Pack v2。不依賴 TW 版本。
- **patch 用 index 重導向，不改任何原始 `NNNN.blob`**，分四段：
  1. **掃描**（平行，每個 blob 一個 task）：記錄每個含 AudioClip 的 bundle 的 (slot, bundle 名稱, {路徑: SHA1})。與 pack 無關，存在 `<data>/scan_cache.json`，以 (blob 路徑, size, mtime) 為鍵，之後只重掃有變的 blob（`--rescan` 強制全掃）。
  2. **規劃**（記憶體）：bundle 內任一 clip 的 JP SHA1 與 TW 不同就列入。`--dry-run` 到此為止。
  3. **重建**（平行，每個 bundle 一個 task）：worker 回傳重打包後的 bundle，主行程依抵達順序直接 append 進 `<pkg>/9876.blob.mmvoice.tmp`，最後補 offset 表。沒有暫存 blob；任何失敗都在動到遊戲目錄前中止並刪掉 .tmp。
  4. **提交**：新 blob 用目前 index 沒在用的編號（9876／9877 交替，`--blob-no N` 可改）→ 原子替換 `index` → 最後才刪舊 blob。
  - 第一次改某個 package 的 index 前，原地複製一份 `<pkg>/index.bak`（另在 `--data/backup` 也有一份）。只有原始 index 會被備份：index 內容若已有指向 ≥5000 號 blob 的條目，就視為已 patch，不備份並中止（例如換了 `--data` 找不到 state 時）。restore 後 `index.bak` 保留。
  - 實套 TW（2026-11-27）：快取掃描下整個 patch 7.7 秒（重建 255 bundle 7 秒）→ 57 blob / 255 bundle / 6749 clip；`AssetPackages/9876.blob` 343 MB（9726 條目）、`AssetPackages/Starter/9876.blob` 22 MB（264 條目）。[確認] 重讀 9876 內 6756 個 clip 全部與 pack 一致；`index.bak` 與原檔 SHA1 相同。[確認] 實機測試成功（2026-11-27）：遊戲接受 9876.blob 與改寫後的 index，NPC 語音為 JP。
  - 新 blob 內 slot 順序是抵達順序，所以每次產生的檔案 bytes 不保證相同，但每個 index 條目指到的 bundle 內容相同。
  - [確認] metadata 中檔名格式字串為 `{0:0000}.blob`，所以新檔名只能是數字，無法用 `JP-NNNN.blob`。
  - 還原＝放回原 `index`（並刪掉新增的 blob）。launcher 修復或遊戲更新換掉 `index` 時 mod 自然失效，再跑一次 patch 即可。
  - patch 每次都從原始 index 備份重算，**重跑冪等**。
- `state.json` 記錄原始／patched index 的 SHA1 與產生的 redirect blob 清單；restore 只還原仍是我們寫入的 index。
- [確認] 離線驗證（假安裝目錄）：97 個 index 條目被改寫，其餘 bytes（含路徑表）完全相同；redirect blob 內 84 個 clip 與 pack 一致；restore 後 index SHA1 與原檔相同。
- 實測（2026-11-27，16 核 `--jobs 14`）：dump 156 秒；patch dry-run 約 180 秒 → 63 blob / 289 bundle / 7023 clip。

## 發佈架構：語音放 Cloudflare Pages、patcher 放 GitHub Releases

```
  本機（你）                                   Cloudflare Pages（JP 語音，不進 git）
  tools/dump_site.py  ── work/pages/<日期>/ ──▶  latest.json / manifest.json / clips/<sha1>
  tools/deploy_pages.ps1 (wrangler)                         ▲ 只下載需要的 clip
                                                           │
  git tag vX.Y.Z ──▶ GitHub Actions ──▶ Release ──▶ 朋友的 patcher（mmvoice.exe，NSIS 安裝、免管理員）
                     (.github/workflows/release.yml)   latest.json＋簽章 ──▶ 程式內自我更新
```

### 語音站台（`tools/dump_site.py` → `tools/deploy_pages.ps1`）

```powershell
.\.venv\Scripts\python.exe tools\dump_site.py D:\Nexon\MabinogiMobile_JP          # → work\pages\<今天日期>\
.\tools\deploy_pages.ps1 work\pages\2026-11-27                                     # wrangler pages deploy
```

- 每次 dump 產生一個全新的日期目錄：`latest.json`（日期＋manifest SHA1）、`manifest.json`（version 3：每個 clip 的大小與 AudioClip 欄位、bundle 名稱＋路徑 → sha1、路徑預設版本）、`clips/<sha1>`（去重後的 JP 原始 FSB5）、`_headers`（clips 永久快取、json 不快取）。
- [確認] 2026-11-27：129 秒，7297 路徑、7380 個 clip、95.6 MB、7383 個檔案（Pages 單次部署上限 20000、單檔 25 MiB）。
- Cloudflare 設定只在本機：`deploy.local.json`（`{"project": "..."}`）、`npx wrangler login` 或 `CLOUDFLARE_API_TOKEN`；`.gitignore` 已排除 `deploy.local.json`、`.wrangler/`、`*.key`。

### patcher：`installer/`（Rust＋Tauri 2）

```
installer/
  core/   mmv-core：UnityFS（讀 LZ4/LZ4HC、寫 LZ4）、SerializedFile v22＋type tree、AudioClip 原地改寫、
          blob / index、語音站台下載與快取、掃描快取 → 規劃 → 下載 → 平行重建 → 安全提交
  cli/    mmvoice-rs.exe：patch <tw_root> <站台網址或本機 dump 目錄> / restore / status
  app/    Tauri GUI（mmvoice.exe）：ui/ 是純 HTML/JS，沒有 bundler；package.json 只為了 Tauri CLI
  build.ps1  本機開發建置（-Source 指定語音站台、-Bundle 產生未簽章的 NSIS 安裝檔）
```

- 語音來源：編譯時的 `MMVOICE_SOURCE`（release workflow 從 repository variable 注入）；執行時可用同名環境變數覆寫（測試用，可指向本機 dump 目錄）。
- 套用流程：讀 `latest.json` → `manifest.json`（驗 SHA1）→ 掃描本機 TW → 規劃 → **只下載要換、且快取（`%LOCALAPPDATA%\mmvoice\voices`）沒有的 clip**（8 條並行，逐檔驗 SHA1）→ 重建 → 提交。離線時用快取的 manifest。套用完清掉新版本用不到的快取。
- 演算法與 `tools/mmvoice.py` 相同，差別：
  - **狀態跟著安裝目錄走**：index 有指向 ≥5000 號 blob 的條目＝已套用；原始 index 一律在 `<pkg>/index.bak`（只會從原始 index 寫入）；package 目錄內 ≥5000 號的 blob 都是我們的。
  - **是否需要重新套用**：每次套用在 index 旁寫 `<pkg>/index.mmvoice.json`（語音版本 id＝manifest SHA1、日期、寫入的 index SHA1、原始 index SHA1、被導向的每個原始 blob 的 size＋mtime）。GUI 依此判斷：
    - index 被換回原檔（遊戲更新／launcher 修復）→「遊戲更新後已還原為韓語」；
    - index 沒變但被導向的原始 blob 有變 →「遊戲檔案已更新」，[推論] 這時 9876 裡是舊版 bundle，必須重新套用；
    - 站台的語音版本和已套用的不同 →「有新的日文語音」；沒有標記 →「版本不明」。
  - 只重建有換到 JP 的 resource 串流，其他串流原封不動；AudioClip 欄位原地覆寫（[確認] 255 個 bundle、7161 個 AudioClip 換完後物件長度都不變）。
  - 輸出沿用實機驗證過的 UnityPy 格式（flags 0xC2：LZ4、128 KiB 分塊、block info 在尾端；不會變小的分塊存原始資料）。
  - 遊戲是否執行中：以寫入模式開這個安裝目錄的 `MabinogiMobile.exe`，sharing violation 就是在跑（不改任何東西）。
- 自我更新：`tauri-plugin-updater`，來源是 `https://github.com/<repo>/releases/latest/download/latest.json`，安裝檔需簽章；公鑰與 endpoint 由 workflow 以 `--config` 注入，repo 裡的 `tauri.conf.json` 只有空白預設值。NSIS 以 currentUser 安裝（`%LOCALAPPDATA%\MabiMobile JP Voice`），更新不需要管理員權限。
- [確認] 驗證（2026-11-27）：
  - 真實 TW 掃描（只讀）：Rust 5.8 秒（Python 121 秒）；2084 bundle / 15179 clip 的名稱、路徑、SHA1 與 Python 完全一致。
  - 57 個相關 blob 全量 patch：4.1 秒；與已實機驗證的 Python 版 `9876.blob` 逐條比對 254619 個 index 條目，255 個重建 bundle 的每個物件全部相同。
  - 從本機 HTTP 站台下載（166 段）→ 套用，結果同樣逐物件一致；重跑零下載；伺服器關閉時用快取 manifest。
  - 自我更新端到端（一次性測試金鑰＋本機伺服器）：安裝 0.1.0 → 偵測 0.1.1 → 下載、驗章、安裝、重啟成 0.1.1 → 套用成功。
  - 重跑（9876→9877）、模擬遊戲更新（index 換回、或只改 blob）、還原，都正確。
- 本機建置工具：scoop 的 rustup（`stable-x86_64-pc-windows-gnu`）＋ scoop 的 `mingw`（提供 `dlltool`）。[確認] GNU toolchain 下 `WebView2Loader.dll` 是動態載入，本機 build 的 exe 要和它放在一起；release 在 GitHub Actions 用 MSVC 編，沒有這個問題。

### 第一次發佈要做的設定

1. GitHub：建 repo、`git remote add origin …`、push。
2. 更新簽章金鑰（只做一次，私鑰不要遺失也不要進 git）：
   `cd installer\app; npx tauri signer generate -w $HOME\.tauri\mmvoice.key`
   → secrets `TAURI_SIGNING_PRIVATE_KEY`（私鑰檔內容）、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；variable `TAURI_UPDATER_PUBKEY`（`.pub` 內容）。
3. Cloudflare Pages：`npx wrangler login`；`npx wrangler pages project create <名稱> --production-branch main`；repo 根目錄建 `deploy.local.json` `{"project": "<名稱>"}`；dump＋deploy。
   → variable `MMVOICE_SOURCE` = `https://<名稱>.pages.dev`。
4. 發版：`installer/Cargo.toml` 的 `version` 改成 X.Y.Z → commit → `git tag vX.Y.Z` → `git push --tags`。

### 舊流程（開發與比對用）

`tools/mmvoice.py dump/patch`（`.mmvpack`）與 `tools/build.py`＋`apply.py` 仍保留作為參考實作與比對基準；給朋友的版本不再使用 `.mmvpack`。

### Pack v2：路徑＋內容（2026-11-27）

- [確認] 兩版同一個 bundle 的 `AssetBundle.m_Name`（如 `dedicated_bundle_7dbd4a02`）相同，所以 TW 的每個 clip 副本會拿「JP 同一個 bundle 裡的那份」；JP 沒有該 bundle 時才用該路徑最常見的 JP 版本。
- 每種 JP 內容依 SHA1 只存一份（manifest 用 sha1 指向同一份資料）。
- 已移除 blacklist：patch 時本來就逐一比對 SHA1，相同的會自動跳過。
- 實測：dump 123 秒 → 7297 路徑（87 個有多個 JP 版本）、7380 份內容、101 MB；patch → 64 blob / 283 bundle / 7236 clip，`9876.blob` 約 380 MB。
- [確認] 9876.blob 比 pack 大，原因是 index 以「整個 bundle」為單位定址：同一段音訊被多個 bundle 內嵌時，每個 bundle 都必須自帶一份，遊戲端無法共用。
- 安全性：單一 bundle 掃描出錯只跳過並警告；新 blob 先寫好（9876/9877 交替）→ 原子替換 index → 最後才刪舊 blob；`--dry-run` 不重新打包。
## 移植流程（第一版，離線驗證用）

1. `scan_audio.py` ×2 + `compare.py` → `work/port_map.tsv`（JP↔TW 對照，每個 TW 實例一行）。
2. `build.py`：對每個 TW bundle 重建 `.resource` 音訊串流（替換的 clip 用 JP 原始 FSB5 bytes，其餘保留），改寫每個 AudioClip 的 `m_Resource.m_Offset/m_Size` 與格式欄位，LZ4 重新打包；再重建 blob 的 offset 表。輸出到 `out/<package>/<blob>`。
   - [確認] 73 blob / 308 bundle / 7362 clip，16 核 18 秒；重掃 `out/` 後 7362 個實例的大小與長度全部等於 JP。
3. [推論] `index` 不用改：它記的是 (blob, bundle 序號)，bundle 數量與順序不變。
4. 實機測試（2026-11-27）：[確認] 只換 `AssetPackages/0260.blob`（凱汀、諾拉），以 launcher 正常啟動可進遊戲，NPC 語音已變成 JP，沒有異常。`tools/apply.py apply|restore` 負責備份／套用／還原。

### 實機前已知風險

- [確認] launcher 的 `59826-obd-manifest`（zlib 壓縮 JSON，檔名 base64）記錄每個檔案每 4 MiB 分塊的 SHA1；改過的 blob 在「修復」或更新時一定會被判定為不符。[推論] 只要不按修復，平常啟動可能不會檢查，需實測。
- [確認] UnityPy 重新打包後的 bundle header flags 與原檔不同（原 `0x243` LZ4HC + 開頭 padding，新 `0xC2` LZ4 + block info 在尾端）。[推論] Unity 2021.3 兩種都能讀。
- BlackCipher 或遊戲本身是否驗證 blob：未知。

## 目錄

```
tools/   解析與轉換腳本
  blob.py          .blob 容器讀取
  scan_audio.py    多進程掃描整個安裝的 AudioClip → TSV（--jobs N，16 核約 2 分鐘）
  compare.py       JP/TW 清單比對，輸出移植對照表
  mmvoice.py       dumper / patcher / restore / status（主要入口）
  patch_bundle.py  單一 bundle 的 AudioClip 替換
  build.py         平行建出所有替換後的 blob 到 out/
  proto_one.py     單一 bundle 原型與驗證
  extract_clip.py  從指定 bundle 抽出 AudioClip 為 WAV
docs/    調查筆記
work/    中間產物（掃描 TSV、樣本），不進 git
out/     輸出（打包好的 mod），不進 git
```

## 環境

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe tools\scan_audio.py D:\MabinogiMobile_TW\appdata work\tw_audio.tsv --jobs 14
.\.venv\Scripts\python.exe tools\scan_audio.py D:\Nexon\MabinogiMobile_JP work\jp_audio.tsv --jobs 14
.\.venv\Scripts\python.exe tools\compare.py work\jp_audio.tsv work\tw_audio.tsv work\port_map.tsv
```

注意：JP launcher 更新／修復時會重寫 blob 並暫時刪除 `index`，掃描前確認 launcher 沒在下載。
