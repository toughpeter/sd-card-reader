use std::{
    fs,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{PairingManifest, PairingResult},
};

const PAIRING_DIRECTORY: &str = ".sd-photo-importer";
const MANIFEST_FILE: &str = "pairing.json";
const SIGNATURE_FILE: &str = "pairing.sig";
const KEY_FILE: &str = "issuer-signing-key";

pub fn load_or_create_signing_key(app_data_dir: &Path) -> AppResult<SigningKey> {
    fs::create_dir_all(app_data_dir).map_err(|error| AppError::io(app_data_dir, error))?;
    let key_path = app_data_dir.join(KEY_FILE);
    if key_path.exists() {
        let encoded =
            fs::read_to_string(&key_path).map_err(|error| AppError::io(&key_path, error))?;
        let bytes = STANDARD
            .decode(encoded.trim())
            .map_err(|error| AppError::PairingInvalid(format!("簽署金鑰無法解碼：{error}")))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| AppError::PairingInvalid("簽署金鑰長度錯誤".to_string()))?;
        return Ok(SigningKey::from_bytes(&bytes));
    }

    // MVP key persistence. Production builds will replace this adapter with
    // macOS Keychain and Windows protected credential storage.
    let key = SigningKey::generate(&mut OsRng);
    let temporary = key_path.with_extension("tmp");
    fs::write(&temporary, STANDARD.encode(key.to_bytes()))
        .map_err(|error| AppError::io(&temporary, error))?;
    restrict_key_permissions(&temporary)?;
    fs::rename(&temporary, &key_path).map_err(|error| AppError::io(&key_path, error))?;
    Ok(key)
}

#[cfg(unix)]
fn restrict_key_permissions(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| AppError::io(path, error))
}

#[cfg(not(unix))]
fn restrict_key_permissions(_path: &Path) -> AppResult<()> {
    Ok(())
}

pub fn initialize_pairing(
    source_root: &Path,
    display_name: String,
    allowed_formats: Vec<String>,
    signing_key: &SigningKey,
) -> AppResult<PairingResult> {
    let pairing_directory = source_root.join(PAIRING_DIRECTORY);
    let existing = verify_pairing(source_root, &signing_key.verifying_key()).ok();
    fs::create_dir_all(&pairing_directory)
        .map_err(|error| AppError::io(&pairing_directory, error))?;

    let manifest = PairingManifest {
        version: 1,
        card_id: existing
            .as_ref()
            .map(|manifest| manifest.card_id.clone())
            .unwrap_or_else(|| format!("CARD-{}", Uuid::new_v4().simple())),
        display_name,
        volume_fingerprint: fingerprint_source(source_root)?,
        fingerprint_kind: "development-volume-path-v1".to_string(),
        allowed_formats,
        created_at: existing
            .map(|manifest| manifest.created_at)
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        issuer_id: issuer_id(&signing_key.verifying_key()),
    };

    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| AppError::PairingInvalid(error.to_string()))?;
    let signature = signing_key.sign(&manifest_bytes);

    atomic_write(&pairing_directory.join(MANIFEST_FILE), &manifest_bytes)?;
    atomic_write(
        &pairing_directory.join(SIGNATURE_FILE),
        STANDARD.encode(signature.to_bytes()).as_bytes(),
    )?;

    let verified = verify_pairing(source_root, &signing_key.verifying_key())?;
    Ok(PairingResult {
        manifest: verified,
        pairing_directory: pairing_directory.to_string_lossy().into_owned(),
    })
}

pub fn verify_pairing(
    source_root: &Path,
    verifying_key: &VerifyingKey,
) -> AppResult<PairingManifest> {
    let pairing_directory = source_root.join(PAIRING_DIRECTORY);
    let manifest_path = pairing_directory.join(MANIFEST_FILE);
    let signature_path = pairing_directory.join(SIGNATURE_FILE);
    if !manifest_path.is_file() || !signature_path.is_file() {
        return Err(AppError::PairingMissing);
    }

    let manifest_bytes =
        fs::read(&manifest_path).map_err(|error| AppError::io(&manifest_path, error))?;
    let encoded_signature = fs::read_to_string(&signature_path)
        .map_err(|error| AppError::io(&signature_path, error))?;
    let signature_bytes = STANDARD
        .decode(encoded_signature.trim())
        .map_err(|_| AppError::SignatureInvalid)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| AppError::SignatureInvalid)?;
    verifying_key
        .verify(&manifest_bytes, &signature)
        .map_err(|_| AppError::SignatureInvalid)?;

    let manifest: PairingManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| AppError::PairingInvalid(error.to_string()))?;
    if manifest.version != 1 {
        return Err(AppError::PairingInvalid(format!(
            "不支援的配對版本：{}",
            manifest.version
        )));
    }
    if manifest.issuer_id != issuer_id(verifying_key) {
        return Err(AppError::SignatureInvalid);
    }
    if manifest.volume_fingerprint != fingerprint_source(source_root)? {
        return Err(AppError::FingerprintMismatch);
    }
    Ok(manifest)
}

fn issuer_id(verifying_key: &VerifyingKey) -> String {
    blake3::hash(verifying_key.as_bytes()).to_hex()[..16].to_ascii_uppercase()
}

fn fingerprint_source(source_root: &Path) -> AppResult<String> {
    let canonical =
        fs::canonicalize(source_root).map_err(|error| AppError::io(source_root, error))?;
    let metadata = fs::metadata(&canonical).map_err(|error| AppError::io(&canonical, error))?;
    let device_identity = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            format!("|dev:{}", metadata.dev())
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            String::new()
        }
    };
    let input = format!(
        "{}|{}{}",
        std::env::consts::OS,
        canonical.to_string_lossy(),
        device_identity
    );

    Ok(blake3::hash(input.as_bytes()).to_hex().to_string())
}

fn atomic_write(path: &Path, contents: &[u8]) -> AppResult<()> {
    let temporary: PathBuf = path.with_extension("tmp");
    fs::write(&temporary, contents).map_err(|error| AppError::io(&temporary, error))?;
    fs::rename(&temporary, path).map_err(|error| AppError::io(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_round_trip_and_tamper_detection() {
        let root = tempfile::tempdir().expect("temp dir");
        fs::create_dir(root.path().join("DCIM")).expect("DCIM");
        let key = SigningKey::generate(&mut OsRng);
        initialize_pairing(
            root.path(),
            "相機 A / SD-01".to_string(),
            vec!["jpg".to_string()],
            &key,
        )
        .expect("pair card");

        let manifest = verify_pairing(root.path(), &key.verifying_key()).expect("verify card");
        assert_eq!(manifest.display_name, "相機 A / SD-01");

        let original_card_id = manifest.card_id;
        initialize_pairing(
            root.path(),
            "相機 A / SD-01".to_string(),
            vec!["jpg".to_string(), "arw".to_string()],
            &key,
        )
        .expect("update card formats");
        let updated = verify_pairing(root.path(), &key.verifying_key()).expect("verify update");
        assert_eq!(updated.card_id, original_card_id);
        assert_eq!(updated.allowed_formats, ["jpg", "arw"]);

        let path = root.path().join(PAIRING_DIRECTORY).join(MANIFEST_FILE);
        let mut bytes = fs::read(&path).expect("read manifest");
        bytes.push(b' ');
        fs::write(path, bytes).expect("tamper manifest");
        assert!(matches!(
            verify_pairing(root.path(), &key.verifying_key()),
            Err(AppError::SignatureInvalid)
        ));
    }
}
