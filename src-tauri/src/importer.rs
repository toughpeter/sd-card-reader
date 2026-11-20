use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

use chrono::{Local, Utc};
use rusqlite::{Connection, OpenFlags, params};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    media::{scan_source, source_path},
    models::{FailedFile, ImportListItem, ImportReport, ImportedFile, PairingManifest},
};

pub fn import_photos(
    source_root: &Path,
    destination_root: &Path,
    manifest: &PairingManifest,
) -> AppResult<ImportReport> {
    let started_at = Utc::now();
    let job_id = Uuid::new_v4().to_string();
    let database_path = database_path(destination_root);
    let scan = scan_unimported(source_root, &manifest.allowed_formats, destination_root)?;
    if scan.candidates.is_empty() {
        let items = scan
            .rejected
            .iter()
            .map(|file| ImportListItem {
                file_name: file.file_name.clone(),
                created_time: file.created_time.clone(),
                is_importable: false,
                import_succeeded: None,
                result_detail: file.reason.clone(),
            })
            .collect();
        return Ok(ImportReport {
            job_id,
            card_id: manifest.card_id.clone(),
            imported: Vec::new(),
            skipped_duplicates: Vec::new(),
            failed: Vec::new(),
            rejected: scan.rejected,
            items,
            total_imported_bytes: 0,
            previously_imported_count: scan.previously_imported_count,
            started_at: started_at.to_rfc3339(),
            completed_at: Utc::now().to_rfc3339(),
        });
    }
    let control_root = destination_root.join(".sd-importer-data");
    let staging_root = control_root.join("staging").join(&job_id);
    fs::create_dir_all(&staging_root).map_err(|error| AppError::io(&staging_root, error))?;
    hide_control_directory(&control_root)?;

    let connection = open_database(&database_path)?;
    let import_folder = destination_root.join(format!("{}匯入", Local::now().format("%Y%m%d")));
    let mut imported = Vec::new();
    let mut skipped_duplicates = Vec::new();
    let mut failed = Vec::new();
    let mut items = Vec::new();
    let mut total_imported_bytes = 0_u64;

    for candidate in &scan.candidates {
        match import_one(
            &connection,
            &staging_root,
            &import_folder,
            manifest,
            candidate,
        ) {
            Ok(ImportOneResult::Imported(file)) => {
                total_imported_bytes = total_imported_bytes.saturating_add(file.size);
                items.push(ImportListItem {
                    file_name: candidate.file_name.clone(),
                    created_time: Some(candidate.created_time.clone()),
                    is_importable: true,
                    import_succeeded: Some(true),
                    result_detail: "匯入成功".to_string(),
                });
                imported.push(file);
            }
            Ok(ImportOneResult::Duplicate(path)) => {
                items.push(ImportListItem {
                    file_name: candidate.file_name.clone(),
                    created_time: Some(candidate.created_time.clone()),
                    is_importable: true,
                    import_succeeded: Some(false),
                    result_detail: "未匯入：內容重複".to_string(),
                });
                skipped_duplicates.push(path);
            }
            Err(error) => {
                let reason = error.to_string();
                items.push(ImportListItem {
                    file_name: candidate.file_name.clone(),
                    created_time: Some(candidate.created_time.clone()),
                    is_importable: true,
                    import_succeeded: Some(false),
                    result_detail: reason.clone(),
                });
                failed.push(FailedFile {
                    source_relative_path: candidate.relative_path.clone(),
                    reason,
                });
            }
        }
    }

    items.extend(scan.rejected.iter().map(|file| ImportListItem {
        file_name: file.file_name.clone(),
        created_time: file.created_time.clone(),
        is_importable: false,
        import_succeeded: None,
        result_detail: file.reason.clone(),
    }));

    if failed.is_empty() {
        let _ = fs::remove_dir_all(&staging_root);
    }

    Ok(ImportReport {
        job_id,
        card_id: manifest.card_id.clone(),
        imported,
        skipped_duplicates,
        failed,
        rejected: scan.rejected,
        items,
        total_imported_bytes,
        previously_imported_count: scan.previously_imported_count,
        started_at: started_at.to_rfc3339(),
        completed_at: Utc::now().to_rfc3339(),
    })
}

pub fn scan_unimported(
    source_root: &Path,
    allowed_formats: &[String],
    destination_root: &Path,
) -> AppResult<crate::models::ScanSummary> {
    let mut scan = scan_source(source_root, allowed_formats)?;
    let path = database_path(destination_root);
    if !path.is_file() {
        return Ok(scan);
    }

    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut candidates = Vec::with_capacity(scan.candidates.len());
    let mut total_bytes = 0_u64;
    for candidate in scan.candidates {
        let content_hash = hash_file(&source_path(&candidate))?;
        if already_imported(&connection, &content_hash)? {
            scan.previously_imported_count += 1;
        } else {
            total_bytes = total_bytes.saturating_add(candidate.size);
            candidates.push(candidate);
        }
    }
    scan.candidates = candidates;
    scan.total_bytes = total_bytes;
    Ok(scan)
}

fn database_path(destination_root: &Path) -> PathBuf {
    destination_root
        .join(".sd-importer-data")
        .join("database.sqlite")
}

enum ImportOneResult {
    Imported(ImportedFile),
    Duplicate(String),
}

#[cfg(windows)]
fn hide_control_directory(path: &Path) -> AppResult<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a valid, null-terminated UTF-16 path for the duration of both calls.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }
    // SAFETY: the pointer remains valid and the attribute value preserves all existing flags.
    if unsafe { SetFileAttributesW(wide.as_ptr(), attributes | FILE_ATTRIBUTE_HIDDEN) } == 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn hide_control_directory(_path: &Path) -> AppResult<()> {
    Ok(())
}

fn import_one(
    connection: &Connection,
    staging_root: &Path,
    import_folder: &Path,
    manifest: &PairingManifest,
    candidate: &crate::models::ScanCandidate,
) -> AppResult<ImportOneResult> {
    let source = source_path(candidate);
    let source_hash = hash_file(&source)?;
    if already_imported(connection, &source_hash)? {
        return Ok(ImportOneResult::Duplicate(candidate.relative_path.clone()));
    }

    let staged = staging_root.join(format!(
        "{}-{}",
        Uuid::new_v4(),
        safe_file_name(&candidate.file_name)
    ));
    fs::copy(&source, &staged).map_err(|error| {
        if source.try_exists().unwrap_or(false) {
            AppError::io(&staged, error)
        } else {
            AppError::io(&source, error)
        }
    })?;
    sync_staged_file(&staged)?;

    let destination_hash = hash_file(&staged)?;
    if destination_hash != source_hash {
        return Err(AppError::PairingInvalid(format!(
            "複製後完整性驗證失敗：{}",
            candidate.relative_path
        )));
    }

    fs::create_dir_all(import_folder).map_err(|error| AppError::io(import_folder, error))?;

    let destination =
        collision_safe_destination(import_folder, &candidate.file_name, &source_hash)?;
    if destination.exists() {
        let _ = fs::remove_file(&staged);
        return Ok(ImportOneResult::Duplicate(candidate.relative_path.clone()));
    }
    fs::rename(&staged, &destination).map_err(|error| AppError::io(&destination, error))?;

    connection.execute(
        "INSERT INTO imported_files (
            content_hash, card_id, source_relative_path, source_size,
            capture_time, destination_path, imported_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            source_hash,
            manifest.card_id,
            candidate.relative_path,
            candidate.size as i64,
            candidate.capture_time,
            destination.to_string_lossy(),
            Utc::now().to_rfc3339(),
        ],
    )?;

    Ok(ImportOneResult::Imported(ImportedFile {
        source_relative_path: candidate.relative_path.clone(),
        destination_path: destination.to_string_lossy().into_owned(),
        content_hash: source_hash,
        size: candidate.size,
    }))
}

fn sync_staged_file(path: &Path) -> AppResult<()> {
    // Windows' FlushFileBuffers requires a handle opened with write access.
    // File::open creates a read-only handle, which makes sync_all fail with
    // ERROR_ACCESS_DENIED even though the preceding copy succeeded.
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| AppError::io(path, error))
}

fn open_database(path: &Path) -> AppResult<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    }
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS imported_files (
            content_hash TEXT PRIMARY KEY,
            card_id TEXT NOT NULL,
            source_relative_path TEXT NOT NULL,
            source_size INTEGER NOT NULL,
            capture_time TEXT NOT NULL,
            destination_path TEXT NOT NULL,
            imported_at TEXT NOT NULL
         );",
    )?;
    Ok(connection)
}

fn already_imported(connection: &Connection, content_hash: &str) -> AppResult<bool> {
    let mut statement = connection
        .prepare("SELECT EXISTS(SELECT 1 FROM imported_files WHERE content_hash = ?1 LIMIT 1)")?;
    Ok(statement.query_row([content_hash], |row| row.get(0))?)
}

fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = File::open(path).map_err(|error| AppError::io(path, error))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| AppError::io(path, error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collision_safe_destination(folder: &Path, file_name: &str, hash: &str) -> AppResult<PathBuf> {
    let safe_name = safe_file_name(file_name);
    let desired = folder.join(&safe_name);
    if !desired.exists() {
        return Ok(desired);
    }
    if hash_file(&desired)? == hash {
        return Ok(desired);
    }

    let path = Path::new(&safe_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("photo");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let candidate_name = if extension.is_empty() {
        format!("{stem}_{}", &hash[..8])
    } else {
        format!("{stem}_{}.{}", &hash[..8], extension)
    };
    Ok(folder.join(candidate_name))
}

fn safe_file_name(value: &str) -> String {
    let path = Path::new(value);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("photo");
    safe_component(file_name)
}

fn safe_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();
    let trimmed = sanitized.trim().trim_matches('.');
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_jpeg(path: &Path, payload: &[u8]) {
        let mut file = File::create(path).expect("jpeg");
        file.write_all(&[0xFF, 0xD8, 0xFF, 0xE0]).expect("header");
        file.write_all(payload).expect("payload");
        file.write_all(&[0xFF, 0xD9]).expect("end");
    }

    #[test]
    fn imports_and_deduplicates_without_changing_source() {
        let source = tempfile::tempdir().expect("source");
        let destination = tempfile::tempdir().expect("destination");
        let dcim = source.path().join("DCIM").join("100CAMERA");
        fs::create_dir_all(&dcim).expect("DCIM");
        let photo = dcim.join("IMG_0001.JPG");
        write_jpeg(&photo, b"photo payload");
        let source_before = fs::read(&photo).expect("source bytes");

        let manifest = PairingManifest {
            version: 1,
            card_id: "CARD-TEST".to_string(),
            display_name: "Camera A".to_string(),
            volume_fingerprint: "test".to_string(),
            fingerprint_kind: "test".to_string(),
            allowed_formats: vec!["jpg".to_string()],
            created_at: Utc::now().to_rfc3339(),
            issuer_id: "test".to_string(),
        };

        let first =
            import_photos(source.path(), destination.path(), &manifest).expect("first import");
        assert_eq!(first.imported.len(), 1);
        assert!(first.failed.is_empty());
        let import_folder = destination
            .path()
            .join(format!("{}匯入", Local::now().format("%Y%m%d")));
        assert!(import_folder.join("IMG_0001.JPG").is_file());
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].import_succeeded, Some(true));

        let second =
            import_photos(source.path(), destination.path(), &manifest).expect("second import");
        assert_eq!(second.imported.len(), 0);
        assert!(second.skipped_duplicates.is_empty());
        assert_eq!(second.previously_imported_count, 1);
        assert!(second.items.is_empty());
        assert_eq!(fs::read(&photo).expect("source after"), source_before);
    }

    #[test]
    fn staged_file_can_be_opened_for_durable_sync() {
        let directory = tempfile::tempdir().expect("staging directory");
        let staged = directory.path().join("staged-photo.jpg");
        fs::write(&staged, b"photo payload").expect("staged file");

        sync_staged_file(&staged).expect("sync staged file");
        assert_eq!(fs::read(staged).expect("staged bytes"), b"photo payload");
    }

    #[test]
    fn scan_only_returns_files_not_previously_imported() {
        let source = tempfile::tempdir().expect("source");
        let destination = tempfile::tempdir().expect("destination");
        let dcim = source.path().join("DCIM").join("100CAMERA");
        fs::create_dir_all(&dcim).expect("DCIM");
        write_jpeg(&dcim.join("IMG_0001.JPG"), b"first photo");

        let manifest = PairingManifest {
            version: 1,
            card_id: "CARD-SCAN".to_string(),
            display_name: "Camera".to_string(),
            volume_fingerprint: "test".to_string(),
            fingerprint_kind: "test".to_string(),
            allowed_formats: vec!["jpg".to_string()],
            created_at: Utc::now().to_rfc3339(),
            issuer_id: "test".to_string(),
        };

        import_photos(source.path(), destination.path(), &manifest).expect("first import");
        write_jpeg(&dcim.join("IMG_0002.JPG"), b"second photo");

        let scan = scan_unimported(source.path(), &manifest.allowed_formats, destination.path())
            .expect("scan");
        assert_eq!(scan.previously_imported_count, 1);
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].file_name, "IMG_0002.JPG");
        assert_eq!(scan.total_bytes, scan.candidates[0].size);
    }

    #[test]
    fn empty_import_does_not_create_control_data() {
        let source = tempfile::tempdir().expect("source");
        let destination = tempfile::tempdir().expect("destination");
        let dcim = source.path().join("DCIM");
        fs::create_dir_all(&dcim).expect("DCIM");
        fs::write(dcim.join("NOT_A_PHOTO.TXT"), b"text").expect("text file");
        let manifest = PairingManifest {
            version: 1,
            card_id: "CARD-EMPTY".to_string(),
            display_name: "Camera".to_string(),
            volume_fingerprint: "test".to_string(),
            fingerprint_kind: "test".to_string(),
            allowed_formats: vec!["jpg".to_string()],
            created_at: Utc::now().to_rfc3339(),
            issuer_id: "test".to_string(),
        };

        let report = import_photos(source.path(), destination.path(), &manifest).expect("import");
        assert!(report.imported.is_empty());
        assert_eq!(report.rejected.len(), 1);
        assert!(!destination.path().join(".sd-importer-data").exists());
    }
}
