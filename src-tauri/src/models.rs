use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SupportedFormat {
    pub extension: String,
    pub label: String,
    pub category: String,
    pub brand: String,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanCandidate {
    pub source_path: String,
    pub relative_path: String,
    pub file_name: String,
    pub extension: String,
    pub size: u64,
    pub created_time: String,
    pub capture_time: String,
    pub capture_date: String,
    pub capture_time_source: CaptureTimeSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CaptureTimeSource {
    Exif,
    FileModified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RejectedFile {
    pub relative_path: String,
    pub file_name: String,
    pub created_time: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub source_root: String,
    pub candidates: Vec<ScanCandidate>,
    pub rejected: Vec<RejectedFile>,
    pub total_bytes: u64,
    pub previously_imported_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingManifest {
    pub version: u32,
    pub card_id: String,
    pub display_name: String,
    pub volume_fingerprint: String,
    pub fingerprint_kind: String,
    pub allowed_formats: Vec<String>,
    pub created_at: String,
    pub issuer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingResult {
    pub manifest: PairingManifest,
    pub pairing_directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub source_root: String,
    pub destination_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFile {
    pub source_relative_path: String,
    pub destination_path: String,
    pub content_hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FailedFile {
    pub source_relative_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportListItem {
    pub file_name: String,
    pub created_time: Option<String>,
    pub is_importable: bool,
    pub import_succeeded: Option<bool>,
    pub result_detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub job_id: String,
    pub card_id: String,
    pub imported: Vec<ImportedFile>,
    pub skipped_duplicates: Vec<String>,
    pub failed: Vec<FailedFile>,
    pub rejected: Vec<RejectedFile>,
    pub items: Vec<ImportListItem>,
    pub total_imported_bytes: u64,
    pub previously_imported_count: usize,
    pub started_at: String,
    pub completed_at: String,
}
