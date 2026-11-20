#[cfg(any(target_os = "macos", windows))]
mod eject;
mod error;
mod importer;
mod media;
mod models;
mod pairing;
mod settings;
#[cfg(any(target_os = "macos", windows))]
mod watcher;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ed25519_dalek::SigningKey;
use models::{
    ImportReport, ImportRequest, PairingManifest, PairingResult, ScanSummary, SupportedFormat,
};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_autostart::MacosLauncher;

struct AppState {
    signing_key: Mutex<SigningKey>,
    app_data_dir: PathBuf,
    import_lock: Arc<Mutex<()>>,
}

#[tauri::command]
fn default_allowed_formats() -> Vec<String> {
    media::default_allowed_formats()
}

#[tauri::command]
fn supported_formats() -> Vec<SupportedFormat> {
    media::supported_formats()
}

#[tauri::command]
async fn scan_card(
    source_path: String,
    destination_path: String,
    allowed_formats: Option<Vec<String>>,
) -> Result<ScanSummary, String> {
    let formats = allowed_formats.unwrap_or_else(media::default_allowed_formats);
    tauri::async_runtime::spawn_blocking(move || {
        importer::scan_unimported(
            &PathBuf::from(source_path),
            &formats,
            &PathBuf::from(destination_path),
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error::AppError::BackgroundTask(error.to_string()).to_string())?
}

#[tauri::command]
async fn initialize_card(
    source_path: String,
    display_name: String,
    allowed_formats: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<PairingResult, String> {
    let signing_key = state
        .signing_key
        .lock()
        .map_err(|_| "無法取得簽署金鑰".to_string())?
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        pairing::initialize_pairing(
            &PathBuf::from(source_path),
            display_name,
            allowed_formats,
            &signing_key,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error::AppError::BackgroundTask(error.to_string()).to_string())?
}

#[tauri::command]
async fn verify_card(
    source_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<PairingManifest, String> {
    let verifying_key = state
        .signing_key
        .lock()
        .map_err(|_| "無法取得驗證金鑰".to_string())?
        .verifying_key();
    tauri::async_runtime::spawn_blocking(move || {
        pairing::verify_pairing(&PathBuf::from(source_path), &verifying_key)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error::AppError::BackgroundTask(error.to_string()).to_string())?
}

#[tauri::command]
async fn import_card(
    request: ImportRequest,
    state: tauri::State<'_, AppState>,
) -> Result<ImportReport, String> {
    let verifying_key = state
        .signing_key
        .lock()
        .map_err(|_| "無法取得驗證金鑰".to_string())?
        .verifying_key();
    let import_lock = Arc::clone(&state.import_lock);
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = import_lock
            .lock()
            .map_err(|_| "匯入工作鎖定失敗".to_string())?;
        let source = PathBuf::from(&request.source_root);
        let destination = PathBuf::from(&request.destination_root);
        let manifest =
            pairing::verify_pairing(&source, &verifying_key).map_err(|error| error.to_string())?;
        importer::import_photos(&source, &destination, &manifest).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error::AppError::BackgroundTask(error.to_string()).to_string())?
}

#[tauri::command]
fn save_destination(
    destination_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    settings::save_destination(&state.app_data_dir, &PathBuf::from(destination_path))
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_saved_destination(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    settings::load_destination(&state.app_data_dir)
        .map(|path| path.map(|value| value.to_string_lossy().into_owned()))
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn eject_card(source_path: String) -> Result<bool, String> {
    #[cfg(any(target_os = "macos", windows))]
    {
        tauri::async_runtime::spawn_blocking(move || {
            eject::eject_volume(&PathBuf::from(source_path))
                .map(|()| true)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error::AppError::BackgroundTask(error.to_string()).to_string())?
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = source_path;
        Ok(false)
    }
}

fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(
        app,
        "open",
        "開啟 SD Card Photo Importer",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "完全結束", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("SD Card Photo Importer")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let app_data_dir = app.path().app_data_dir()?;
            let signing_key = pairing::load_or_create_signing_key(&app_data_dir)?;
            let import_lock = Arc::new(Mutex::new(()));
            app.manage(AppState {
                signing_key: Mutex::new(signing_key.clone()),
                app_data_dir: app_data_dir.clone(),
                import_lock: Arc::clone(&import_lock),
            });
            #[cfg(any(target_os = "macos", windows))]
            watcher::start(app.handle().clone(), app_data_dir, signing_key, import_lock);
            create_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let tauri::WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            default_allowed_formats,
            supported_formats,
            scan_card,
            initialize_card,
            verify_card,
            import_card,
            eject_card,
            save_destination,
            get_saved_destination,
        ])
        .run(tauri::generate_context!())
        .expect("error while running SD Card Photo Importer");
}
