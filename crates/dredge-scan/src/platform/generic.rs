//! Portable fallback built on `std::fs`. Allocated size is estimated from the logical size.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use dredge_core::model::{FLAG_ESTIMATED, FLAG_LOSSY};
use dredge_core::{Kind, RawEntry};

use super::DirErr;

const ESTIMATE_CLUSTER: u64 = 4096;

pub fn classify(e: &std::io::Error) -> DirErr {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return DirErr::Denied;
    }
    // Windows ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION.
    if cfg!(windows) && matches!(e.raw_os_error(), Some(32) | Some(33)) {
        return DirErr::Locked;
    }
    DirErr::Other
}

#[cfg_attr(unix, allow(dead_code))]
pub fn read_dir(path: &Path, out: &mut Vec<RawEntry>) -> Result<(), DirErr> {
    let rd = fs::read_dir(path).map_err(|e| classify(&e))?;
    let mut result = Ok(());
    for ent in rd {
        let ent = match ent {
            Ok(e) => e,
            Err(e) => {
                result = Err(classify(&e));
                continue;
            }
        };
        let os_name = ent.file_name();
        let md = match ent.metadata() {
            Ok(m) => m,
            Err(e) => {
                result = Err(classify(&e));
                continue;
            }
        };
        let ft = md.file_type();
        let kind = if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        let mut flags = 0;
        if os_name.to_str().is_none() {
            flags |= FLAG_LOSSY;
        }
        let (allocated, logical) = if kind == Kind::File {
            flags |= FLAG_ESTIMATED;
            let len = md.len();
            (len.div_ceil(ESTIMATE_CLUSTER) * ESTIMATE_CLUSTER, len)
        } else {
            (0, 0)
        };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs().min(u64::from(u32::MAX)) as u32)
            .unwrap_or(0);
        let mut e = RawEntry::new(
            os_name.to_string_lossy().into_owned(),
            kind,
            allocated,
            logical,
        );
        e.flags = flags;
        e.mtime = mtime;
        out.push(e);
    }
    result
}
