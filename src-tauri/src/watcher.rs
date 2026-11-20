use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[cfg(target_os = "macos")]
use std::fs;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

use ed25519_dalek::SigningKey;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{eject, importer, pairing, settings};

const EVENT_NAME: &str = "auto-import-status";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoImportStatus {
    state: &'static str,
    title: String,
    detail: String,
    imported_count: usize,
    rejected_count: usize,
    ejected: bool,
}

pub fn start(
    app: AppHandle,
    app_data_dir: PathBuf,
    signing_key: SigningKey,
    import_lock: Arc<Mutex<()>>,
) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        let mut seen = HashSet::new();
        loop {
            let current = mounted_volumes();
            for volume in current.difference(&seen) {
                process_volume(&app, volume, &app_data_dir, &signing_key, &import_lock);
            }
            seen = current;
            thread::sleep(Duration::from_secs(2));
        }
    });
}

#[cfg(target_os = "macos")]
fn mounted_volumes() -> HashSet<PathBuf> {
    fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

#[cfg(windows)]
fn mounted_volumes() -> HashSet<PathBuf> {
    const DRIVE_REMOVABLE: u32 = 2;

    // SAFETY: GetLogicalDrives takes no pointers and has no preconditions.
    let drive_mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|index| drive_mask & (1 << index) != 0)
        .filter_map(|index| {
            let letter = b'A' + index as u8;
            let root_wide = [letter as u16, b':' as u16, b'\\' as u16, 0];
            // SAFETY: root_wide is a valid, NUL-terminated drive-root string.
            let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
            (drive_type == DRIVE_REMOVABLE).then(|| PathBuf::from(format!("{}:\\", letter as char)))
        })
        .collect()
}

fn emit(app: &AppHandle, status: AutoImportStatus) {
    let _ = app.emit(EVENT_NAME, status);
}

fn process_volume(
    app: &AppHandle,
    volume: &Path,
    app_data_dir: &Path,
    signing_key: &SigningKey,
    import_lock: &Arc<Mutex<()>>,
) {
    let manifest = match pairing::verify_pairing(volume, &signing_key.verifying_key()) {
        Ok(manifest) => manifest,
        Err(_) => return,
    };

    emit(
        app,
        AutoImportStatus {
            state: "detected",
            title: format!("已偵測到 {}", manifest.display_name),
            detail: "配對簽章驗證通過，準備自動匯入。".to_string(),
            imported_count: 0,
            rejected_count: 0,
            ejected: false,
        },
    );

    let destination = match settings::load_destination(app_data_dir) {
        Ok(Some(path)) => path,
        Ok(None) => {
            emit(
                app,
                AutoImportStatus {
                    state: "needsDestination",
                    title: "需要設定匯入位置".to_string(),
                    detail: "請開啟程式選擇一次目的根目錄；設定後重新插入記憶卡。".to_string(),
                    imported_count: 0,
                    rejected_count: 0,
                    ejected: false,
                },
            );
            return;
        }
        Err(error) => {
            emit_error(app, "目的資料夾無法使用", error.to_string());
            return;
        }
    };

    emit(
        app,
        AutoImportStatus {
            state: "importing",
            title: "正在自動匯入".to_string(),
            detail: format!("來源：{}", manifest.display_name),
            imported_count: 0,
            rejected_count: 0,
            ejected: false,
        },
    );

    let _guard = match import_lock.lock() {
        Ok(guard) => guard,
        Err(_) => {
            emit_error(app, "自動匯入無法開始", "匯入工作鎖定失敗".to_string());
            return;
        }
    };
    let report = match importer::import_photos(volume, &destination, &manifest) {
        Ok(report) => report,
        Err(error) => {
            emit_error(app, "自動匯入失敗", error.to_string());
            return;
        }
    };

    if !report.failed.is_empty() {
        emit(
            app,
            AutoImportStatus {
                state: "error",
                title: "自動匯入部分失敗".to_string(),
                detail: format!(
                    "成功 {} 個、失敗 {} 個；記憶卡未退出。",
                    report.imported.len(),
                    report.failed.len()
                ),
                imported_count: report.imported.len(),
                rejected_count: report.rejected.len(),
                ejected: false,
            },
        );
        return;
    }

    let handled = report.imported.len() + report.previously_imported_count;
    if handled == 0 {
        emit(
            app,
            AutoImportStatus {
                state: "error",
                title: "沒有可自動匯入的媒體".to_string(),
                detail: format!(
                    "{} 個檔案被格式或安全政策阻擋；記憶卡未退出。",
                    report.rejected.len()
                ),
                imported_count: 0,
                rejected_count: report.rejected.len(),
                ejected: false,
            },
        );
        return;
    }

    match eject::eject_volume(volume) {
        Ok(()) => emit(
            app,
            AutoImportStatus {
                state: "completed",
                title: "自動匯入與安全退出完成".to_string(),
                detail: format!(
                    "新匯入 {} 個；先前已匯入 {} 個。現在可以拔除記憶卡。",
                    report.imported.len(),
                    report.previously_imported_count
                ),
                imported_count: report.imported.len(),
                rejected_count: report.rejected.len(),
                ejected: true,
            },
        ),
        Err(error) => emit_error(app, "匯入完成，但安全退出失敗", error.to_string()),
    }
}

fn emit_error(app: &AppHandle, title: &str, detail: String) {
    emit(
        app,
        AutoImportStatus {
            state: "error",
            title: title.to_string(),
            detail,
            imported_count: 0,
            rejected_count: 0,
            ejected: false,
        },
    );
}
