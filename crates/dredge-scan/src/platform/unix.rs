//! Unix backend: `lstat` per entry. Allocated size is `st_blocks * 512`; hard links come from
//! `st_nlink` and `st_ino`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use dredge_core::model::{FLAG_LOSSY, FLAG_MOUNT};
use dredge_core::{Kind, RawEntry};

use super::generic::classify;
use super::DirErr;

pub fn volume_info(root: &Path) -> (Option<String>, Option<u64>) {
    match fs::metadata(root) {
        Ok(md) => (Some(format!("{:x}", md.dev())), Some(md.dev())),
        Err(_) => (None, None),
    }
}

pub fn read_dir(path: &Path, root_dev: Option<u64>, out: &mut Vec<RawEntry>) -> Result<(), DirErr> {
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
        // DirEntry::metadata does not follow symlinks on Unix.
        let md = match ent.metadata() {
            Ok(m) => m,
            Err(e) => {
                result = Err(classify(&e));
                continue;
            }
        };
        let os_name = ent.file_name();
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
        if kind == Kind::Dir && root_dev.is_some_and(|d| d != md.dev()) {
            flags |= FLAG_MOUNT;
        }
        let mut e = RawEntry::new(
            os_name.to_string_lossy().into_owned(),
            kind,
            md.blocks().saturating_mul(512),
            md.len(),
        );
        e.flags = flags;
        e.mtime = md.mtime().clamp(0, i64::from(u32::MAX)) as u32;
        if kind == Kind::File {
            e.nlink = Some(md.nlink().min(u64::from(u32::MAX)) as u32);
            e.file_id = Some(u128::from(md.ino()));
        }
        out.push(e);
    }
    result
}
