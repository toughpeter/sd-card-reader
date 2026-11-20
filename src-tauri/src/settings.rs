use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    destination_root: String,
}

pub fn save_destination(app_data_dir: &Path, destination: &Path) -> AppResult<PathBuf> {
    let canonical =
        fs::canonicalize(destination).map_err(|error| AppError::io(destination, error))?;
    if !canonical.is_dir() {
        return Err(AppError::InvalidDestination(canonical));
    }
    fs::create_dir_all(app_data_dir).map_err(|error| AppError::io(app_data_dir, error))?;
    let settings = AppSettings {
        destination_root: canonical.to_string_lossy().into_owned(),
    };
    let contents = serde_json::to_vec_pretty(&settings)
        .map_err(|error| AppError::PairingInvalid(error.to_string()))?;
    let path = app_data_dir.join(SETTINGS_FILE);
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents).map_err(|error| AppError::io(&temporary, error))?;
    fs::rename(&temporary, &path).map_err(|error| AppError::io(&path, error))?;
    Ok(canonical)
}

pub fn load_destination(app_data_dir: &Path) -> AppResult<Option<PathBuf>> {
    let path = app_data_dir.join(SETTINGS_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let contents = fs::read(&path).map_err(|error| AppError::io(&path, error))?;
    let settings: AppSettings = serde_json::from_slice(&contents)
        .map_err(|error| AppError::PairingInvalid(format!("目的資料夾設定無效：{error}")))?;
    let destination = PathBuf::from(settings.destination_root);
    if !destination.is_dir() {
        return Err(AppError::InvalidDestination(destination));
    }
    Ok(Some(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_round_trip() {
        let app_data = tempfile::tempdir().expect("app data");
        let destination = tempfile::tempdir().expect("destination");
        let saved = save_destination(app_data.path(), destination.path()).expect("save");
        assert_eq!(
            load_destination(app_data.path()).expect("load"),
            Some(saved)
        );
    }
}
