use std::path::PathBuf;

/// Top-level error type for all CCE crates.
#[derive(Debug, thiserror::Error)]
pub enum CceError {
    /// The path is not a usable repository root.
    #[error("invalid repository path: {0}")]
    InvalidRepository(PathBuf),
    /// A resolved path escapes the repository root (traversal guard).
    #[error("path escapes repository root: {0}")]
    PathEscape(PathBuf),
    /// A byte/line range does not map onto the referenced file.
    #[error("invalid source range for {path}: {start_byte}..{end_byte}")]
    InvalidSourceRange {
        /// File path the range was applied to.
        path: String,
        /// Start byte of the offending range.
        start_byte: u64,
        /// End byte of the offending range.
        end_byte: u64,
    },
    /// A requested view is unavailable or partial for the given reason.
    #[error("view {view} is not available: {reason}")]
    ViewUnavailable {
        /// View name (e.g. `graph`, `dataflow`).
        view: String,
        /// Why the view cannot serve the request.
        reason: String,
    },
    /// A content-addressed artifact is missing or failed integrity check.
    #[error("artifact {0} is missing or corrupt")]
    ArtifactCorrupt(String),
    /// On-disk data was written by an incompatible CCE version.
    #[error("unsupported data format version {found}; maximum supported is {supported}")]
    UnsupportedFormat {
        /// Version found on disk.
        found: u32,
        /// Highest version this build understands.
        supported: u32,
    },
    /// Configuration or environment problem.
    #[error("configuration error: {0}")]
    Configuration(String),
    /// Filesystem failure at a specific path.
    #[error("I/O error at {path}: {source}")]
    Io {
        /// Path whose operation failed.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// JSON (de)serialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// SQLite/metadata-store failure.
    #[error("storage error: {0}")]
    Storage(String),
    /// Embedding backend failure.
    #[error("embedding error: {0}")]
    Embedding(String),
    /// The repository's index lease is held by another operation.
    #[error("another index operation owns the repository lease at {0}")]
    IndexBusy(PathBuf),
    /// The operation was cancelled by the caller.
    #[error("operation cancelled")]
    Cancelled,
}

/// Convenience alias for CCE results.
pub type Result<T, E = CceError> = std::result::Result<T, E>;

impl CceError {
    /// Wrap an `io::Error` with the path it occurred at.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
