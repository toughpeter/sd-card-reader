import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { open } from "@tauri-apps/plugin-dialog";

type PairingManifest = { cardId: string; displayName: string; allowedFormats: string[] };
type ScanSummary = { candidates: Array<{ relativePath: string; size: number }>; rejected: Array<{ relativePath: string; reason: string }>; totalBytes: number; previouslyImportedCount: number };
type ImportListItem = { fileName: string; createdTime: string | null; isImportable: boolean; importSucceeded: boolean | null; resultDetail: string };
type ImportReport = { imported: Array<{ destinationPath: string; size: number }>; skippedDuplicates: string[]; failed: Array<{ sourceRelativePath: string; reason: string }>; rejected: Array<{ relativePath: string; reason: string }>; items: ImportListItem[]; totalImportedBytes: number; previouslyImportedCount: number };
type SupportedFormat = { extension: string; label: string; category: "standard" | "raw" | "video"; brand: string; defaultEnabled: boolean };
type AutoImportStatus = { state: "detected" | "needsDestination" | "importing" | "completed" | "error"; title: string; detail: string; importedCount: number; rejectedCount: number; ejected: boolean };

const state: { sourcePath: string | null; destinationPath: string | null; manifest: PairingManifest | null; scan: ScanSummary | null; busy: boolean } = { sourcePath: null, destinationPath: null, manifest: null, scan: null, busy: false };
let supportedFormatsCatalog: SupportedFormat[] = [];

function element<T extends HTMLElement>(selector: string): T { const found = document.querySelector<T>(selector); if (!found) throw new Error(`Missing element: ${selector}`); return found; }
function setText(selector: string, value: string) { element(selector).textContent = value; }
function errorMessage(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function formats(): string[] { return Array.from(document.querySelectorAll<HTMLInputElement>('input[name="format"]:checked')).map((input) => input.value); }
function selectFormats(allowed: string[]) {
  const selected = new Set(allowed.map((format) => format.toLowerCase()));
  document.querySelectorAll<HTMLInputElement>('input[name="format"]').forEach((input) => { input.checked = selected.has(input.value); });
  updateFormatSummary();
}
function rejectionSummary(rejected: Array<{ reason: string }>): string {
  const reasons = Array.from(new Set(rejected.map((file) => file.reason)));
  if (!reasons.length) return "沒有找到媒體檔案；請確認記憶卡包含 DCIM、AVCHD、M4ROOT 或 CONTENTS 資料夾。";
  const preview = reasons.slice(0, 2).join("；");
  return `${rejected.length} 個檔案被阻擋：${preview}${reasons.length > 2 ? "；另有其他原因" : ""}`;
}
function updateFormatSummary() {
  const total = document.querySelectorAll<HTMLInputElement>('input[name="format"]').length;
  const selected = formats().length;
  setText("#format-summary", `已允許 ${selected} / ${total} 種格式，涵蓋照片、RAW 與相機影片`);
  setText("#toggle-all-formats", selected === total ? "取消全選" : "全部選取");
}
function formatOption(format: SupportedFormat): HTMLLabelElement {
  const label = document.createElement("label");
  const input = document.createElement("input");
  const copy = document.createElement("span");
  const name = document.createElement("strong");
  const brand = document.createElement("small");
  input.type = "checkbox"; input.name = "format"; input.value = format.extension; input.checked = format.defaultEnabled;
  name.textContent = `.${format.extension.toUpperCase()} · ${format.label}`; brand.textContent = format.brand;
  copy.append(name, brand); label.append(input, copy); input.addEventListener("change", updateFormatSummary);
  return label;
}
async function configureFormats() {
  try {
    const catalog = await invoke<SupportedFormat[]>("supported_formats");
    supportedFormatsCatalog = catalog;
    const standard = element("#standard-formats"); const raw = element("#raw-formats"); const video = element("#video-formats");
    catalog.forEach((format) => (format.category === "raw" ? raw : format.category === "video" ? video : standard).append(formatOption(format)));
    updateFormatSummary();
    element("#toggle-all-formats").addEventListener("click", () => {
      const inputs = Array.from(document.querySelectorAll<HTMLInputElement>('input[name="format"]'));
      const shouldSelect = inputs.some((input) => !input.checked);
      inputs.forEach((input) => { input.checked = shouldSelect; }); updateFormatSummary();
    });
  } catch (error) {
    setText("#format-summary", `無法讀取格式清單：${errorMessage(error)}`);
    element<HTMLButtonElement>("#initialize-card").disabled = true;
  }
}
function formatBytes(bytes: number): string { if (!bytes) return "0 B"; const units = ["B", "KB", "MB", "GB", "TB"]; const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1); return `${(bytes / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`; }
async function configureAppVersion() {
  try {
    const version = await getVersion();
    setText("#app-version", `v${version}`);
    document.title = `SD Card Media Importer v${version}`;
  } catch (error) {
    setText("#app-version", "版本未知");
    setText("#activity-log", `無法讀取應用程式版本：${errorMessage(error)}`);
  }
}
async function configureSavedDestination() {
  try {
    const saved = await invoke<string | null>("get_saved_destination");
    if (saved) { state.destinationPath = saved; setText("#destination-path", saved); }
  } catch (error) {
    setText("#activity-log", `無法讀取目的資料夾設定：${errorMessage(error)}`);
  } finally { refreshActions(); }
}
async function configureAutoImportEvents() {
  await listen<AutoImportStatus>("auto-import-status", ({ payload }) => {
    state.busy = payload.state === "importing";
    setText("#service-label", payload.state === "importing" ? "自動匯入中" : payload.state === "error" || payload.state === "needsDestination" ? "需要處理" : "背景監聽中");
    setText("#console-title", payload.title);
    setText("#console-detail", payload.detail);
    setText("#activity-log", payload.detail);
    document.body.classList.toggle("is-busy", state.busy);
    if (payload.state === "completed") {
      setText("#metric-files", String(payload.importedCount));
      setText("#metric-rejected", String(payload.rejectedCount));
      if (payload.ejected) {
        state.sourcePath = null; state.manifest = null; state.scan = null;
        setText("#source-path", "記憶卡已安全退出");
        setTrust("waiting", "自動匯入完成", "記憶卡已安全退出，可以拔除。");
      }
    }
    refreshActions();
  });
}
function formatDateTime(value: string | null): string {
  if (!value) return "無法取得";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat("zh-TW", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(date);
}
function statusBadge(label: string, kind: "success" | "danger" | "muted"): HTMLSpanElement {
  const badge = document.createElement("span");
  badge.className = `result-badge ${kind}`;
  badge.textContent = label;
  return badge;
}
function renderImportList(report: ImportReport) {
  const body = element<HTMLTableSectionElement>("#import-list-body");
  body.replaceChildren();
  report.items.forEach((item) => {
    const row = document.createElement("tr");
    const fileName = document.createElement("td");
    const createdTime = document.createElement("td");
    const importable = document.createElement("td");
    const succeeded = document.createElement("td");
    fileName.textContent = item.fileName;
    fileName.title = item.fileName;
    createdTime.textContent = formatDateTime(item.createdTime);
    importable.append(statusBadge(item.isImportable ? "是" : "否", item.isImportable ? "success" : "danger"));
    const result = item.importSucceeded === true
      ? statusBadge("成功", "success")
      : item.importSucceeded === false
        ? statusBadge("未成功", "danger")
        : statusBadge("不適用", "muted");
    result.title = item.resultDetail;
    succeeded.append(result);
    if (item.resultDetail) {
      const detail = document.createElement("small");
      detail.textContent = item.resultDetail;
      succeeded.append(detail);
    }
    row.append(fileName, createdTime, importable, succeeded);
    body.append(row);
  });
  setText("#import-list-summary", `共 ${report.items.length} 個檔案 · 成功 ${report.imported.length} · 未成功 ${report.failed.length + report.skippedDuplicates.length} · 不可匯入 ${report.rejected.length}`);
  element<HTMLElement>("#import-list").hidden = false;
}

function setBusy(busy: boolean, message?: string) { state.busy = busy; document.body.classList.toggle("is-busy", busy); if (message) setText("#activity-log", message); refreshActions(); }
function refreshActions() {
  element<HTMLButtonElement>("#verify-card").disabled = !state.sourcePath || state.busy;
  element<HTMLButtonElement>("#initialize-card").disabled = !state.sourcePath || state.busy;
  element<HTMLButtonElement>("#scan-card").disabled = !state.sourcePath || !state.destinationPath || !state.manifest || state.busy;
  element<HTMLButtonElement>("#import-card").disabled = !state.manifest || !state.destinationPath || !state.scan?.candidates.length || state.busy;
}
function setTrust(kind: "waiting" | "trusted" | "unknown" | "error", title: string, detail: string) {
  element("#trust-panel").setAttribute("data-state", kind); setText("#trust-title", title); setText("#trust-detail", detail); setText(".trust-icon", kind === "trusted" ? "✓" : kind === "unknown" ? "!" : kind === "error" ? "×" : "—");
}

async function chooseDirectory(target: "source" | "destination") {
  const selected = await open({ directory: true, multiple: false }); if (!selected || Array.isArray(selected)) return;
  if (target === "source") { state.sourcePath = selected; state.manifest = null; state.scan = null; selectFormats(supportedFormatsCatalog.filter((format) => format.defaultEnabled).map((format) => format.extension)); setText("#initialize-card", "信任並初始化"); element<HTMLElement>("#import-list").hidden = true; setText("#source-path", selected); setTrust("waiting", "尚未驗證", "按下「驗證配對」檢查這張記憶卡。"); setText("#console-title", "來源已選擇"); setText("#console-detail", "請先驗證記憶卡配對狀態。"); }
  else {
    try {
      const saved = await invoke<string>("save_destination", { destinationPath: selected });
      state.destinationPath = saved; state.scan = null; setText("#destination-path", saved);
      setText("#activity-log", "目的根目錄已儲存，後續插入已配對卡片會自動匯入");
    } catch (error) { setText("#activity-log", `無法儲存目的資料夾：${errorMessage(error)}`); }
  }
  refreshActions();
}

async function verifyCard() {
  if (!state.sourcePath) return; setBusy(true, "正在驗證配對簽章…");
  try { state.manifest = await invoke<PairingManifest>("verify_card", { sourcePath: state.sourcePath }); element<HTMLInputElement>("#card-name").value = state.manifest.displayName; selectFormats(state.manifest.allowedFormats); setText("#initialize-card", "更新格式白名單"); setTrust("trusted", "已信任", `${state.manifest.displayName} · ${state.manifest.cardId}`); setText("#console-title", "記憶卡驗證完成"); setText("#console-detail", `目前允許 ${state.manifest.allowedFormats.length} 種格式；需要時可更新白名單。`); }
  catch (error) { state.manifest = null; const detail = errorMessage(error); const missing = detail.includes("配對檔不存在"); setTrust(missing ? "unknown" : "error", missing ? "未知記憶卡" : "驗證失敗", detail); setText("#console-title", missing ? "等待首次信任" : "已阻擋記憶卡"); setText("#console-detail", detail); }
  finally { setBusy(false, "驗證完成"); }
}

async function initializeCard() {
  if (!state.sourcePath) return; const updating = !!state.manifest; const displayName = element<HTMLInputElement>("#card-name").value.trim(); if (!displayName) { setText("#activity-log", "請輸入卡片名稱"); return; } setBusy(true, updating ? "正在更新簽章格式白名單…" : "正在建立簽章配對檔…");
  try { const result = await invoke<{ manifest: PairingManifest }>("initialize_card", { sourcePath: state.sourcePath, displayName, allowedFormats: formats() }); state.manifest = result.manifest; setText("#initialize-card", "更新格式白名單"); setTrust("trusted", updating ? "白名單已更新" : "初始化完成", `${result.manifest.displayName} · ${result.manifest.cardId}`); setText("#console-title", updating ? "格式白名單更新完成" : "這張卡已加入白名單"); setText("#console-detail", `已允許 ${result.manifest.allowedFormats.length} 種格式，現在可以重新掃描照片與影片。`); setText("#activity-log", "配對簽章已寫入並回讀驗證"); }
  catch (error) { setTrust("error", "初始化失敗", errorMessage(error)); setText("#activity-log", errorMessage(error)); }
  finally { setBusy(false); }
}

async function scanCard() {
  if (!state.sourcePath || !state.destinationPath || !state.manifest) return; setBusy(true, "正在掃描尚未匯入的照片與影片…"); element<HTMLElement>("#progress-bar").style.width = "36%";
  try { state.scan = await invoke<ScanSummary>("scan_card", { sourcePath: state.sourcePath, destinationPath: state.destinationPath, allowedFormats: state.manifest.allowedFormats }); setText("#metric-files", String(state.scan.candidates.length)); setText("#metric-size", formatBytes(state.scan.totalBytes)); setText("#metric-rejected", String(state.scan.rejected.length)); const empty = state.scan.candidates.length === 0; const allImported = empty && state.scan.previouslyImportedCount > 0 && state.scan.rejected.length === 0; setText("#console-title", allImported ? "沒有新的媒體檔案" : empty ? "沒有可匯入的媒體檔案" : `找到 ${state.scan.candidates.length} 個尚未匯入的檔案`); setText("#console-detail", allImported ? `${state.scan.previouslyImportedCount} 個檔案先前已匯入。` : empty || state.scan.rejected.length ? rejectionSummary(state.scan.rejected) : state.scan.previouslyImportedCount ? `已排除 ${state.scan.previouslyImportedCount} 個先前匯入的檔案。` : "所有候選檔案都通過基本格式驗證。"); element<HTMLElement>("#progress-bar").style.width = "100%"; setText("#activity-log", allImported ? "目的資料夾已包含全部媒體" : empty ? "請檢查格式白名單與相機媒體資料夾" : "掃描完成，等待開始匯入"); }
  catch (error) { state.scan = null; element<HTMLElement>("#progress-bar").style.width = "0"; setText("#console-title", "掃描失敗"); setText("#console-detail", errorMessage(error)); setText("#activity-log", errorMessage(error)); }
  finally { setBusy(false); }
}

async function importCard() {
  if (!state.sourcePath || !state.destinationPath || !state.scan) return; setBusy(true, "正在複製並進行 BLAKE3 完整性驗證…"); element<HTMLElement>("#progress-bar").style.width = "22%";
  try {
    const sourcePath = state.sourcePath;
    const report = await invoke<ImportReport>("import_card", { request: { sourceRoot: sourcePath, destinationRoot: state.destinationPath } });
    element<HTMLElement>("#progress-bar").style.width = "100%";
    const nothingImported = report.imported.length === 0;
    const allImported = nothingImported && report.previouslyImportedCount > 0 && report.failed.length === 0 && report.rejected.length === 0;
    setText("#console-title", allImported ? "沒有新的媒體檔案" : nothingImported ? "沒有媒體檔案被匯入" : `已安全匯入 ${report.imported.length} 個檔案`);
    setText("#console-detail", allImported ? `${report.previouslyImportedCount} 個檔案先前已匯入。` : nothingImported ? rejectionSummary(report.rejected) : `略過 ${report.skippedDuplicates.length} 個重複檔案；失敗 ${report.failed.length} 個；阻擋 ${report.rejected.length} 個。`);
    setText("#metric-files", String(report.imported.length)); setText("#metric-size", formatBytes(report.totalImportedBytes)); setText("#metric-rejected", String(report.rejected.length + report.failed.length));
    setText("#activity-log", allImported ? "目的資料夾已包含全部媒體" : nothingImported ? "未建立匯入資料；請先修正阻擋原因" : report.failed.length ? "匯入部分完成，請檢查失敗項目" : "完整性驗證通過，正在安全退出記憶卡…");
    renderImportList(report); state.scan = null;

    if (report.imported.length > 0 && report.failed.length === 0) {
      try {
        const ejected = await invoke<boolean>("eject_card", { sourcePath });
        if (ejected) {
          setText("#console-title", `已匯入 ${report.imported.length} 個檔案並安全退出`);
          setText("#console-detail", "記憶卡已由 macOS 安全退出，現在可以拔除。");
          setText("#activity-log", "匯入、完整性驗證與安全退出皆已完成");
          setText("#source-path", "記憶卡已安全退出");
          setTrust("waiting", "記憶卡已退出", "可以拔除記憶卡；再次插入後會自動重新驗證。");
          state.sourcePath = null; state.manifest = null;
        }
      } catch (ejectError) {
        setText("#console-title", `已安全匯入 ${report.imported.length} 個檔案`);
        setText("#console-detail", `匯入已完成，但記憶卡未能自動退出：${errorMessage(ejectError)}。請在 Finder 手動退出。`);
        setText("#activity-log", "匯入完成；等待手動安全退出記憶卡");
      }
    }
  }
  catch (error) { setText("#console-title", "匯入失敗"); setText("#console-detail", errorMessage(error)); setText("#activity-log", errorMessage(error)); }
  finally { setBusy(false); }
}

async function configureAutostart() {
  const input = element<HTMLInputElement>("#autostart");
  try { input.checked = await isEnabled(); input.addEventListener("change", async () => { input.disabled = true; try { if (input.checked) await enable(); else await disable(); } catch (error) { input.checked = !input.checked; setText("#activity-log", `自動啟動設定失敗：${errorMessage(error)}`); } finally { input.disabled = false; } }); }
  catch (error) { input.disabled = true; setText("#activity-log", `無法讀取自動啟動設定：${errorMessage(error)}`); }
}

window.addEventListener("DOMContentLoaded", () => {
  element("#select-source").addEventListener("click", () => chooseDirectory("source")); element("#select-destination").addEventListener("click", () => chooseDirectory("destination")); element("#verify-card").addEventListener("click", verifyCard); element("#initialize-card").addEventListener("click", initializeCard); element("#scan-card").addEventListener("click", scanCard); element("#import-card").addEventListener("click", importCard); configureAppVersion(); configureSavedDestination(); configureAutoImportEvents(); configureFormats(); configureAutostart(); refreshActions();
});
