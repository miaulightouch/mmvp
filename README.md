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

- TW 語音是什麼語言（韓語？中文？）——需要人耳聽 `work/samples/tw/` 的樣本。
- monster／UI／bgm 中「大小不同」的少數項目，是在地化差異還是單純版本差異（JP 剛更新過）。
- 改檔後的完整性檢查：BlackCipher（反作弊）與 launcher 的「修復」是否會偵測／覆蓋修改過的 blob。[推論] 很可能會，需實測。

## 預計移植方式（[推論]，尚未實作）

1. 以 container path 建立 JP ↔ TW 對照表（`tools/scan_audio.py` 產出 TSV）。
2. 對每個 TW bundle，把其中 AudioClip 的音訊資料換成 JP 對應 clip 的資料（同為 Vorbis/FSB5，可直接搬原始 resource bytes，並更新 `m_Length`、`m_Resource.m_Size` 等欄位）。
3. 重新打包 bundle → 重建 blob offset table →（如需要）更新 `index`。
4. 輸出到 `out/`，以覆蓋或旁路載入方式測試。

## 目錄

```
tools/   解析與轉換腳本
  blob.py          .blob 容器讀取
  scan_audio.py    多進程掃描整個安裝的 AudioClip → TSV（--jobs N，16 核約 2 分鐘）
  compare.py       JP/TW 清單比對，輸出移植對照表
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
