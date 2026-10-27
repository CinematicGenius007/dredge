use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::Error;
use crate::model::Index;

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// Worker threads; `None` = logical cores.
    pub threads: Option<usize>,
}

/// Live counters a scanner updates and a frontend reads. Also carries the cancel flag.
#[derive(Default)]
pub struct ScanProgress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub allocated_bytes: AtomicU64,
    pub errors: AtomicU64,
    cancel: AtomicBool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ProgressSnapshot {
    pub files: u64,
    pub dirs: u64,
    pub allocated_bytes: u64,
    pub errors: u64,
}

impl ScanProgress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            allocated_bytes: self.allocated_bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A scan backend. `WalkScanner` is the only one in phase 1; a future MFT scanner implements
/// the same trait and feeds `IndexBuilder`, with no change to core or the frontends.
pub trait Scanner: Send + Sync {
    fn name(&self) -> &'static str;

    /// `root` is an absolute, normalized directory path.
    fn scan(&self, root: &Path, opts: &ScanOptions, progress: &ScanProgress) -> Result<Index, Error>;
}
