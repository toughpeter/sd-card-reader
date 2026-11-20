use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("找不到相機媒體資料夾（DCIM、AVCHD、M4ROOT 或 CONTENTS）：{0}")]
    MediaFoldersNotFound(PathBuf),
    #[error("無法存取路徑 {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("配對檔不存在")]
    PairingMissing,
    #[error("配對檔格式無效：{0}")]
    PairingInvalid(String),
    #[error("配對簽章驗證失敗")]
    SignatureInvalid,
    #[error("記憶卡特徵與配對資料不符")]
    FingerprintMismatch,
    #[error("拒絕退出非外接磁碟根目錄：{0}")]
    UnsafeEjectPath(PathBuf),
    #[error("安全退出失敗：{0}")]
    EjectFailed(String),
    #[error("目的資料夾無效或不存在：{0}")]
    InvalidDestination(PathBuf),
    #[error("資料庫錯誤：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("背景工作失敗：{0}")]
    BackgroundTask(String),
}

impl AppError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
