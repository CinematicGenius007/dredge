//! One function per platform: list a directory into `RawEntry` records.

use std::path::Path;

use dredge_core::RawEntry;

mod generic;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirErr {
    Denied,
    Locked,
    Other,
    /// The platform fast path is not available for this directory; use the generic path.
    #[cfg_attr(not(windows), allow(dead_code))]
    Unsupported,
}

/// Per-scan facts a backend may need.
pub struct PlatformCtx {
    /// Unix: device of the scan root, used to avoid crossing filesystems.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub root_dev: Option<u64>,
}

/// Volume identifier of the filesystem holding `root`, plus the device number on Unix.
pub fn volume_info(root: &Path) -> (Option<String>, Option<u64>) {
    #[cfg(windows)]
    {
        (windows::volume_id(root), None)
    }
    #[cfg(unix)]
    {
        unix::volume_info(root)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = root;
        (None, None)
    }
}

/// Entries pushed before an error are kept; the caller decides how to report a partial listing.
#[allow(unused_variables)]
pub fn read_dir(path: &Path, ctx: &PlatformCtx, out: &mut Vec<RawEntry>) -> Result<(), DirErr> {
    #[cfg(windows)]
    {
        match windows::read_dir(path, out) {
            Err(DirErr::Unsupported) if out.is_empty() => generic::read_dir(path, out),
            Err(DirErr::Unsupported) => Err(DirErr::Other),
            r => r,
        }
    }
    #[cfg(unix)]
    {
        unix::read_dir(path, ctx.root_dev, out)
    }
    #[cfg(not(any(windows, unix)))]
    {
        generic::read_dir(path, out)
    }
}
