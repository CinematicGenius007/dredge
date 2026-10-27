use thiserror::Error;

/// Every failure the engine can report. `code()` is the stable string used in JSON error documents.
#[derive(Debug, Error)]
pub enum Error {
    #[error("no snapshot covers {path}; run `dredge scan` on it first")]
    NoSnapshot { path: String },
    #[error("{path} is not in the snapshot of {root} (new since the scan, or it does not exist)")]
    PathNotInSnapshot { path: String, root: String },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("scan failed: {0}")]
    ScanFailed(String),
    #[error("snapshot {path} is corrupt ({reason}); rescan")]
    SnapshotCorrupt { path: String, reason: String },
    #[error("snapshot {path} has unsupported format version {found}; rescan")]
    SnapshotVersion { path: String, found: u32 },
    #[error("job not found: {0}")]
    JobNotFound(String),
    #[error("I/O error ({context}): {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::NoSnapshot { .. } => "no_snapshot",
            Error::PathNotInSnapshot { .. } => "path_not_in_snapshot",
            Error::InvalidArgument(_) => "invalid_argument",
            Error::ScanFailed(_) => "scan_failed",
            Error::SnapshotCorrupt { .. } => "snapshot_corrupt",
            Error::SnapshotVersion { .. } => "snapshot_version_unsupported",
            Error::JobNotFound(_) => "job_not_found",
            Error::Io { .. } => "io_error",
            Error::Internal(_) => "internal",
        }
    }

    /// A command the caller can run to fix the problem, when one exists.
    pub fn hint(&self) -> Option<String> {
        match self {
            Error::NoSnapshot { path } => Some(format!("dredge scan {path}")),
            _ => None,
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }
}
