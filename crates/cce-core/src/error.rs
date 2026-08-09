use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CceError {
    #[error("invalid repository path: {0}")]
    InvalidRepository(PathBuf),
    #[error("path escapes repository root: {0}")]
    PathEscape(PathBuf),
    #[error("invalid source range for {path}: {start_byte}..{end_byte}")]
    InvalidSourceRange {
        path: String,
        start_byte: u64,
        end_byte: u64,
    },
    #[error("view {view} is not available: {reason}")]
    ViewUnavailable { view: String, reason: String },
    #[error("artifact {0} is missing or corrupt")]
    ArtifactCorrupt(String),
    #[error("unsupported data format version {found}; maximum supported is {supported}")]
    UnsupportedFormat { found: u32, supported: u32 },
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("another index operation owns the repository lease at {0}")]
    IndexBusy(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
}

pub type Result<T, E = CceError> = std::result::Result<T, E>;

impl CceError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
