use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, NaiveDateTime, Utc};
use exif::{In, Reader, Tag};
use walkdir::WalkDir;

use crate::{
    error::{AppError, AppResult},
    models::{CaptureTimeSource, RejectedFile, ScanCandidate, ScanSummary, SupportedFormat},
};

const STANDARD_FORMATS: &[(&str, &str)] = &[
    ("jpg", "JPEG"),
    ("jpeg", "JPEG（完整副檔名）"),
    ("jpe", "JPEG（相機短副檔名）"),
    ("mpo", "MPO 多視角照片"),
    ("heic", "HEIC"),
    ("heif", "HEIF"),
    ("hif", "HIF 高效率照片"),
    ("png", "PNG"),
    ("tif", "TIFF"),
    ("tiff", "TIFF（完整副檔名）"),
    ("webp", "WebP"),
    ("avif", "AVIF"),
    ("gif", "GIF"),
    ("bmp", "BMP"),
];

const RAW_FORMATS: &[(&str, &str, &str)] = &[
    ("cr2", "CR2", "Canon"),
    ("cr3", "CR3", "Canon"),
    ("crw", "CRW", "Canon（舊機型）"),
    ("nef", "NEF", "Nikon"),
    ("nrw", "NRW", "Nikon Coolpix"),
    ("arw", "ARW", "Sony"),
    ("arq", "ARQ Pixel Shift", "Sony"),
    ("sr2", "SR2", "Sony"),
    ("srf", "SRF", "Sony"),
    ("raf", "RAF", "Fujifilm"),
    ("rw2", "RW2", "Panasonic / Lumix"),
    ("raw", "RAW", "Panasonic / Lumix（舊機型）"),
    ("orf", "ORF", "Olympus / OM System"),
    ("ori", "ORI", "Olympus / OM System"),
    ("pef", "PEF", "Pentax"),
    ("ptx", "PTX", "Pentax"),
    ("dng", "DNG / ProRAW", "Adobe、Apple、Leica、Ricoh 等"),
    ("gpr", "GPR", "GoPro"),
    ("rwl", "RWL", "Leica"),
    ("3fr", "3FR", "Hasselblad"),
    ("fff", "FFF", "Hasselblad / Imacon"),
    ("iiq", "IIQ", "Phase One"),
    ("x3f", "X3F", "Sigma / Foveon"),
    ("srw", "SRW", "Samsung"),
    ("dcr", "DCR", "Kodak"),
    ("kdc", "KDC", "Kodak"),
    ("k25", "K25", "Kodak（舊機型）"),
    ("erf", "ERF", "Epson"),
    ("mrw", "MRW", "Minolta"),
    ("mef", "MEF", "Mamiya"),
    ("mos", "MOS", "Leaf"),
];

const VIDEO_FORMATS: &[(&str, &str, &str)] = &[
    (
        "mov",
        "QuickTime Video",
        "iPhone、Canon、Nikon、Sony、Lumix",
    ),
    ("mp4", "MPEG-4 Video", "Sony、Canon、Nikon、Lumix、iPhone"),
    ("m4v", "MPEG-4 Video", "Apple / iPhone"),
    ("mts", "AVCHD Video", "Sony、Panasonic Lumix、Canon"),
    ("m2ts", "AVCHD Video", "Sony、Panasonic Lumix、Canon"),
    ("avi", "AVI Video", "Nikon、Canon（舊機型）"),
    ("mxf", "MXF Professional Video", "Sony、Canon、Panasonic"),
    ("crm", "Cinema RAW Light", "Canon Cinema EOS"),
];

const MAX_MEDIA_SIZE: u64 = 256 * 1024 * 1024 * 1024;
const MEDIA_DIRECTORIES: &[&str] = &[
    "DCIM",
    "PRIVATE/AVCHD/BDMV/STREAM",
    "PRIVATE/M4ROOT/CLIP",
    "PRIVATE/SONY/PRO/CLIP",
    "CONTENTS/VIDEO",
    "CONTENTS/CLIPS001",
];

pub fn supported_formats() -> Vec<SupportedFormat> {
    STANDARD_FORMATS
        .iter()
        .map(|(extension, label)| SupportedFormat {
            extension: (*extension).to_string(),
            label: (*label).to_string(),
            category: "standard".to_string(),
            brand: "通用".to_string(),
            default_enabled: true,
        })
        .chain(
            RAW_FORMATS
                .iter()
                .map(|(extension, label, brand)| SupportedFormat {
                    extension: (*extension).to_string(),
                    label: (*label).to_string(),
                    category: "raw".to_string(),
                    brand: (*brand).to_string(),
                    default_enabled: true,
                }),
        )
        .chain(
            VIDEO_FORMATS
                .iter()
                .map(|(extension, label, brand)| SupportedFormat {
                    extension: (*extension).to_string(),
                    label: (*label).to_string(),
                    category: "video".to_string(),
                    brand: (*brand).to_string(),
                    default_enabled: true,
                }),
        )
        .collect()
}

pub fn normalize_allowed_formats(formats: &[String]) -> HashSet<String> {
    formats
        .iter()
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

pub fn scan_source(source_root: &Path, allowed_formats: &[String]) -> AppResult<ScanSummary> {
    let media_roots: Vec<PathBuf> = MEDIA_DIRECTORIES
        .iter()
        .map(|relative| source_root.join(relative))
        .filter(|path| path.is_dir())
        .collect();
    if media_roots.is_empty() {
        return Err(AppError::MediaFoldersNotFound(source_root.to_path_buf()));
    }

    let allowed = normalize_allowed_formats(allowed_formats);
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    let mut total_bytes = 0_u64;

    for media_root in media_roots {
        for entry in WalkDir::new(&media_root).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    rejected.push(RejectedFile {
                        relative_path: error
                            .path()
                            .map(|path| relative_display(source_root, path))
                            .unwrap_or_else(|| "DCIM".to_string()),
                        file_name: error
                            .path()
                            .and_then(Path::file_name)
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "DCIM".to_string()),
                        created_time: None,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };

            if entry.file_type().is_symlink() {
                rejected.push(RejectedFile {
                    relative_path: relative_display(source_root, entry.path()),
                    file_name: entry.file_name().to_string_lossy().into_owned(),
                    created_time: entry
                        .metadata()
                        .ok()
                        .and_then(|metadata| created_time(&metadata)),
                    reason: "不允許符號連結".to_string(),
                });
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    rejected.push(RejectedFile {
                        relative_path: relative_display(source_root, path),
                        file_name: entry.file_name().to_string_lossy().into_owned(),
                        created_time: None,
                        reason: format!("無法讀取檔案資訊：{error}"),
                    });
                    continue;
                }
            };
            let file_created_time = created_time(&metadata);
            let extension = extension_of(path);
            if !allowed.contains(&extension) {
                rejected.push(RejectedFile {
                    relative_path: relative_display(source_root, path),
                    file_name: entry.file_name().to_string_lossy().into_owned(),
                    created_time: file_created_time,
                    reason: format!("格式不在白名單：.{extension}"),
                });
                continue;
            }

            if metadata.len() > MAX_MEDIA_SIZE {
                rejected.push(RejectedFile {
                    relative_path: relative_display(source_root, path),
                    file_name: entry.file_name().to_string_lossy().into_owned(),
                    created_time: file_created_time.clone(),
                    reason: "檔案超過 256 GB 安全上限".to_string(),
                });
                continue;
            }

            match has_expected_magic(path, &extension) {
                Ok(true) => {}
                Ok(false) => {
                    rejected.push(RejectedFile {
                        relative_path: relative_display(source_root, path),
                        file_name: entry.file_name().to_string_lossy().into_owned(),
                        created_time: file_created_time.clone(),
                        reason: "副檔名與實際檔案格式不符".to_string(),
                    });
                    continue;
                }
                Err(error) => {
                    rejected.push(RejectedFile {
                        relative_path: relative_display(source_root, path),
                        file_name: entry.file_name().to_string_lossy().into_owned(),
                        created_time: file_created_time.clone(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            }

            let (capture_time, capture_time_source) = capture_time(path, metadata.modified().ok());
            total_bytes = total_bytes.saturating_add(metadata.len());
            candidates.push(ScanCandidate {
                source_path: path.to_string_lossy().into_owned(),
                relative_path: relative_display(source_root, path),
                file_name: entry.file_name().to_string_lossy().into_owned(),
                extension,
                size: metadata.len(),
                created_time: file_created_time.unwrap_or_else(|| capture_time.to_rfc3339()),
                capture_time: capture_time.to_rfc3339(),
                capture_date: capture_time.format("%Y-%m-%d").to_string(),
                capture_time_source,
            });
        }
    }

    candidates.sort_by(|left, right| {
        left.capture_time
            .cmp(&right.capture_time)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });

    Ok(ScanSummary {
        source_root: source_root.to_string_lossy().into_owned(),
        candidates,
        rejected,
        total_bytes,
        previously_imported_count: 0,
    })
}

fn created_time(metadata: &std::fs::Metadata) -> Option<String> {
    metadata
        .created()
        .or_else(|_| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from)
        .map(|time| time.to_rfc3339())
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn has_expected_magic(path: &Path, extension: &str) -> AppResult<bool> {
    let mut file = File::open(path).map_err(|error| AppError::io(path, error))?;
    let mut header = [0_u8; 512];
    let bytes_read = file
        .read(&mut header)
        .map_err(|error| AppError::io(path, error))?;
    let bytes = &header[..bytes_read];

    let is_jpeg = bytes.starts_with(&[0xFF, 0xD8, 0xFF]);
    let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let is_tiff = bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*");
    let is_bigtiff = bytes.starts_with(b"II+\0") || bytes.starts_with(b"MM\0+");
    let is_tiff_family = is_tiff || is_bigtiff;
    let is_raf = bytes.starts_with(b"FUJIFILMCCD-RAW");
    let is_orf = bytes.starts_with(b"IIRO") || bytes.starts_with(b"MMOR");
    let is_rw2 = bytes.starts_with(b"IIU\0");
    let is_crw = bytes.starts_with(b"II") && bytes.windows(8).any(|part| part == b"HEAPCCDR");
    let is_x3f = bytes.starts_with(b"FOVb");
    let is_mrw = bytes.starts_with(b"\0MRM");
    let is_gif = bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a");
    let is_bmp = bytes.starts_with(b"BM");
    let is_webp = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP");
    let is_avi = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"AVI ");
    let is_mxf = bytes.starts_with(&[0x06, 0x0E, 0x2B, 0x34]);
    let is_transport_stream = (bytes.first() == Some(&0x47)
        && bytes.get(188) == Some(&0x47)
        && bytes.get(376) == Some(&0x47))
        || (bytes.get(4) == Some(&0x47)
            && bytes.get(196) == Some(&0x47)
            && bytes.get(388) == Some(&0x47));
    let is_iso_media = bytes.get(4..8) == Some(b"ftyp");
    let has_iso_brand = |brands: &[&[u8; 4]]| {
        is_iso_media
            && bytes
                .get(8..)
                .unwrap_or_default()
                .chunks_exact(4)
                .any(|brand| brands.iter().any(|expected| brand == expected.as_slice()))
    };

    let matches = match extension {
        "jpg" | "jpeg" | "jpe" | "mpo" => is_jpeg,
        "png" => is_png,
        "tif" | "tiff" => is_tiff_family,
        "gif" => is_gif,
        "bmp" => is_bmp,
        "webp" => is_webp,
        "heic" | "heif" | "hif" => has_iso_brand(&[
            b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
        ]),
        "avif" => has_iso_brand(&[b"avif", b"avis"]),
        "mov" => has_iso_brand(&[b"qt  "]),
        "mp4" | "m4v" => has_iso_brand(&[
            b"isom", b"iso2", b"mp41", b"mp42", b"avc1", b"M4V ", b"MSNV", b"XAVC",
        ]),
        "mts" | "m2ts" => is_transport_stream,
        "avi" => is_avi,
        "mxf" => is_mxf,
        "crm" => has_iso_brand(&[b"crx "]),
        "cr2" => is_tiff && bytes.get(8..10) == Some(b"CR"),
        "cr3" => has_iso_brand(&[b"crx "]),
        "crw" => is_crw,
        "raf" => is_raf,
        "orf" | "ori" => is_orf || is_tiff,
        "rw2" | "raw" => is_rw2 || is_tiff,
        "x3f" => is_x3f,
        "mrw" => is_mrw,
        "arw" | "arq" | "dng" | "nef" | "nrw" | "sr2" | "srf" | "pef" | "ptx" | "gpr" | "rwl"
        | "3fr" | "fff" | "iiq" | "srw" | "dcr" | "kdc" | "k25" | "erf" | "mef" | "mos" => {
            is_tiff_family
        }
        _ => false,
    };
    Ok(matches)
}

fn capture_time(path: &Path, modified: Option<SystemTime>) -> (DateTime<Utc>, CaptureTimeSource) {
    if let Ok(file) = File::open(path) {
        let mut reader = BufReader::new(file);
        if let Ok(exif) = Reader::new().read_from_container(&mut reader)
            && let Some(field) = exif.get_field(Tag::DateTimeOriginal, In::PRIMARY)
        {
            let value = field.display_value().with_unit(&exif).to_string();
            if let Ok(parsed) = NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(value.trim(), "%Y:%m:%d %H:%M:%S"))
            {
                return (
                    DateTime::<Utc>::from_naive_utc_and_offset(parsed, Utc),
                    CaptureTimeSource::Exif,
                );
            }
        }
    }

    (
        modified.map(DateTime::<Utc>::from).unwrap_or_else(Utc::now),
        CaptureTimeSource::FileModified,
    )
}

pub fn default_allowed_formats() -> Vec<String> {
    supported_formats()
        .into_iter()
        .filter(|format| format.default_enabled)
        .map(|format| format.extension)
        .collect()
}

pub fn source_path(candidate: &ScanCandidate) -> PathBuf {
    PathBuf::from(&candidate.source_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn header_matches(extension: &str, header: &[u8]) -> bool {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(format!("sample.{extension}"));
        let mut file = File::create(&path).expect("test file");
        file.write_all(header).expect("write header");
        has_expected_magic(&path, extension).expect("read test file")
    }

    #[test]
    fn catalog_extensions_have_a_recognizer() {
        let mut transport_stream = vec![0_u8; 389];
        transport_stream[0] = 0x47;
        transport_stream[188] = 0x47;
        transport_stream[376] = 0x47;
        assert!(header_matches("mts", &transport_stream));
        assert!(header_matches("m2ts", &transport_stream));

        let fixtures: &[(&str, &[u8])] = &[
            ("jpg", b"\xff\xd8\xff"),
            ("jpeg", b"\xff\xd8\xff"),
            ("jpe", b"\xff\xd8\xff"),
            ("mpo", b"\xff\xd8\xff"),
            ("png", b"\x89PNG\r\n\x1a\n"),
            ("tif", b"II*\0"),
            ("tiff", b"MM\0*"),
            ("gif", b"GIF89a"),
            ("bmp", b"BM"),
            ("webp", b"RIFF\0\0\0\0WEBP"),
            ("heic", b"\0\0\0\x18ftypheic\0\0\0\0heic"),
            ("heif", b"\0\0\0\x18ftypmif1\0\0\0\0heif"),
            ("hif", b"\0\0\0\x18ftypmif1\0\0\0\0heic"),
            ("avif", b"\0\0\0\x18ftypavif\0\0\0\0avif"),
            ("mov", b"\0\0\0\x18ftypqt  \0\0\0\0qt  "),
            ("mp4", b"\0\0\0\x18ftypisom\0\0\0\0isom"),
            ("m4v", b"\0\0\0\x18ftypM4V \0\0\0\0M4V "),
            ("avi", b"RIFF\0\0\0\0AVI "),
            ("mxf", b"\x06\x0e\x2b\x34"),
            ("crm", b"\0\0\0\x18ftypcrx \0\0\0\0crx "),
            ("cr2", b"II*\0\0\0\0\0CR"),
            ("cr3", b"\0\0\0\x18ftypcrx \0\0\0\0crx "),
            ("crw", b"II\x1a\0\0\0HEAPCCDR"),
            ("raf", b"FUJIFILMCCD-RAW"),
            ("orf", b"IIRO"),
            ("rw2", b"IIU\0"),
            ("x3f", b"FOVb"),
            ("mrw", b"\0MRM"),
        ];
        for (extension, header) in fixtures {
            assert!(
                header_matches(extension, header),
                ".{extension} fixture was rejected"
            );
        }

        for format in supported_formats() {
            if matches!(format.extension.as_str(), "mts" | "m2ts") {
                continue;
            }
            if fixtures
                .iter()
                .any(|(extension, _)| *extension == format.extension)
            {
                continue;
            }
            assert!(
                header_matches(&format.extension, b"II*\0"),
                ".{} has no TIFF-family recognizer",
                format.extension
            );
        }
    }

    #[test]
    fn executable_content_is_rejected_even_with_media_extension() {
        assert!(!header_matches("jpg", b"MZ executable"));
        assert!(!header_matches("dng", b"MZ executable"));
        assert!(!header_matches("mp4", b"MZ executable"));
    }

    #[test]
    fn defaults_include_standard_raw_and_video_formats() {
        let defaults = default_allowed_formats();
        for extension in [
            "jpg", "heic", "png", "cr2", "cr3", "nef", "arw", "raf", "rw2", "raw", "orf", "pef",
            "dng", "rwl", "3fr", "iiq", "x3f", "mov", "mp4", "mts", "m2ts", "avi", "mxf",
        ] {
            assert!(
                defaults.contains(&extension.to_string()),
                "missing .{extension}"
            );
        }
        assert_eq!(defaults.len(), supported_formats().len());
    }

    #[test]
    fn scans_avchd_video_outside_dcim() {
        let source = tempfile::tempdir().expect("source");
        let stream = source
            .path()
            .join("PRIVATE")
            .join("AVCHD")
            .join("BDMV")
            .join("STREAM");
        std::fs::create_dir_all(&stream).expect("AVCHD stream");
        let mut contents = vec![0_u8; 512];
        contents[0] = 0x47;
        contents[188] = 0x47;
        contents[376] = 0x47;
        std::fs::write(stream.join("00001.MTS"), contents).expect("video");

        let scan = scan_source(source.path(), &["mts".to_string()]).expect("scan video");
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].extension, "mts");
    }
}
