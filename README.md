# mmvp

Mabinogi Mobile Voice Pack (for TW)：把台版《瑪奇 Mobile》的韓語配音換成日版配音。

- 只替換語音，不修改任何原始遊戲檔案，隨時可以還原
- 首次下載約 100 MB，之後只下載有變動的語音
- 遊戲更新或語音更新時會提示重新套用，程式本身會自動更新

## 安裝

1. 從 [Releases](https://github.com/miaulightouch/mmvp/releases/latest) 下載其中一個：
   - `mmvp-portable.exe`：免安裝，放在可寫入的資料夾直接執行
   - `*-setup.exe`：安裝版（不需要系統管理員權限）
2. 關閉遊戲，開啟 mmvp，確認遊戲目錄後按「套用日文語音」。
3. 要恢復原版，按「還原原版語音」。

## 運作方式

遊戲資源是一組 `NNNN.blob`（內含 Unity asset bundle）加上 `index`（資產與相依 bundle 的位置表）。
mmvp 找出含有語音的 bundle，換上日版語音後重新打包成 `9876.blob`，再把 `index` 中所有指向這些 bundle 的位置改到新檔。
原始 `index` 備份為 `index.bak`。

## 手動還原

1. 將 AssetPackages 與 AssetPackages\Starter 中的 index.bak 改回 index
2. 將 AssetPackages 與 AssetPackages\Starter 中的 9876.blob（或 9877.blob）移除

## 開發

```
core/    mmvp-core：UnityFS、SerializedFile、AudioClip、blob / index、語音下載、套用與還原
app/     mmvp：Tauri 2 GUI（ui/ 為純 HTML/JS）
tools/   mmvp_dump.py：從日版 dump 語音；mmvp_publish.ps1：dump 並部署到 Cloudflare Pages
docs/    研究筆記與驗證紀錄
```

```powershell
cargo test -p mmvp-core                      # 單元測試；設 MMVP_TEST_ROOT / MMVP_TEST_SOURCE 可跑端到端測試
.\tools\mmvp_publish.ps1                     # 發佈語音：dump → 有變動才部署
git tag v0.2.0; git push origin v0.2.0       # 發佈程式：版本寫在 Cargo.toml，由 GitHub Actions 建置
```

設定、格式與驗證細節見 [docs/research.md](docs/research.md)。

## 注意

非官方工具，與 NEXON、devCAT 無關。使用風險自負。
