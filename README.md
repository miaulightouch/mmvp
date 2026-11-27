# mmvp

Mabinogi Mobile Voice Pack (for TW)：把台版《瑪奇 Mobile》的韓語配音換成日版配音。

- 只替換語音，不修改任何原始遊戲檔案，隨時可以還原
- 首次下載約 100 MB，之後只下載有變動的語音
- 遊戲更新或語音更新時會提示重新套用，程式本身會自動更新

## 安裝

1. 從 [Releases](https://github.com/miaulightouch/mmvp/releases/latest) 下載其中一個：
   - `mmvoice-portable.exe`：免安裝，放在可寫入的資料夾直接執行
   - `*-setup.exe`：安裝版（不需要系統管理員權限）
2. 關閉遊戲，開啟 **MabiMobile JP Voice**，確認遊戲目錄後按「套用日文語音」。
3. 要恢復原版，按「還原原版語音」。

## 運作方式

遊戲資源是一組 `NNNN.blob`（內含 Unity asset bundle）加上一個 `index` 對照表。
patcher 掃描台版的語音，下載對應的日版語音重新打包成 `9876.blob`，再把 `index` 中對應的條目指向它。
原始 `index` 備份為 `index.bak`。

## 手動還原

1. 將 AssetPackages 與 AssetPackages\Starter 中的 index.bak 改回 index
2. 將 AssetPackages 與 AssetPackages\Starter 中的 9876.blob 移除

## 開發

```
installer/   patcher（Rust + Tauri 2）
  core/      解析與重打包：UnityFS、SerializedFile、AudioClip、blob / index
  cli/       mmvoice-rs：patch / restore / status
  app/       GUI
tools/       Python：語音 dump、Cloudflare Pages 部署、研究用腳本
docs/        研究筆記與驗證紀錄
```

```powershell
# 發佈語音（本機）：dump 日版語音 → 有變動才部署
.\tools\publish_voices.ps1

# 發佈 patcher：版本寫在 installer/Cargo.toml，推 tag 由 GitHub Actions 建置並發佈
git tag v0.1.0; git push origin v0.1.0
```

設定、格式與驗證細節見 [docs/research.md](docs/research.md)。

## 注意

非官方工具，與 NEXON、devCAT 無關。使用風險自負。
