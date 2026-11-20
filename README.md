# SD Card Media Importer

跨平台的相機 SD 卡照片與影片安全匯入工具。產品需求請見 [spec.md](./spec.md)。

## 目前里程碑

目前版本提供可操作的開發模式：使用資料夾模擬 SD 卡，完成首次簽章配對、媒體格式掃描、安全複製、完整性驗證、日期分類與重複排除。

已實作：

- Tauri 2＋TypeScript 桌面介面
- macOS 選單列／Windows 系統匣常駐骨架
- 登入後自動啟動設定
- macOS 採用選單列常駐模式：Dock 不顯示圖示，關閉主視窗後繼續背景運行
- Ed25519 配對檔簽章與竄改偵測
- 53 種照片、相機 RAW 與影片格式，依 Canon、Nikon、Sony、Fujifilm、Panasonic、OM System、Pentax、Leica、Hasselblad、Phase One 等品牌分類
- 支援 DCIM、AVCHD、Sony M4ROOT 與專業相機 CONTENTS 媒體目錄
- 副檔名白名單與檔頭簽章雙重檢查，偽裝成媒體檔的其他檔案不會匯入
- EXIF 拍攝時間與檔案時間後備
- staging 匯入與 BLAKE3 完整性驗證
- SQLite 匯入紀錄與內容雜湊去重；掃描時只列出尚未匯入的檔案
- 依卡片、年份與拍攝日期建立資料夾
- 單元測試確認來源不變及重複媒體不再匯入
- macOS 匯入與完整性驗證成功後自動安全退出外接記憶卡
- macOS 背景監聽已配對記憶卡，並持久保存目的根目錄以自動匯入
- Windows 背景監聽可移除磁碟，已配對記憶卡會自動驗證、匯入並嘗試安全退出

尚未接入：

- macOS 原生 Disk Arbitration 事件整合（目前以 `/Volumes` 背景監聽實作）
- OS Keychain／Windows protected credential 金鑰儲存
- 系統通知與中斷工作復原 UI

## 開發

需求：Node.js、npm、Rust，以及目標平台的 Tauri 系統依賴。

```bash
npm install
npm run tauri dev
```

前端檢查：

```bash
npm run build
```

Rust 測試：

```bash
cd src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
```

## 開發模式操作

1. 建立或選擇一個包含 `DCIM`、`PRIVATE/AVCHD`、`PRIVATE/M4ROOT` 或 `CONTENTS` 的資料夾。
2. 選擇該資料夾作為記憶卡來源。
3. 驗證失敗並顯示未知卡後，按「信任並初始化」。
4. 選擇目的根目錄。
5. 掃描尚未匯入的照片與影片並開始安全匯入。

初始化會在模擬卡片根目錄建立：

```text
.sd-photo-importer/
├── pairing.json
└── pairing.sig
```

除這個初始化操作外，匯入核心只以唯讀方式開啟來源媒體。
