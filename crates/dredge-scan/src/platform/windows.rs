//! Windows backend. One handle per directory, enumerated with
//! `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)`: each entry carries allocation size,
//! logical size, attributes, reparse tag, mtime and the 128-bit file ID in a single bulk call.
//! Files are never opened, so cloud placeholders are never hydrated.

use std::cell::RefCell;
use std::ffi::{c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use dredge_core::model::{FLAG_CLOUD, FLAG_LOSSY};
use dredge_core::{Kind, RawEntry};

use super::DirErr;

// FILE_INFO_BY_HANDLE_CLASS values.
const FILE_ID_INFO: i32 = 18;
const FILE_ID_EXTD_DIRECTORY_INFO: i32 = 19;
const FILE_ID_EXTD_DIRECTORY_RESTART_INFO: i32 = 20;

// Win32 error codes.
const ERROR_INVALID_FUNCTION: u32 = 1;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_SHARING_VIOLATION: u32 = 32;
const ERROR_LOCK_VIOLATION: u32 = 33;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;
const ERROR_INVALID_LEVEL: u32 = 124;

// File attributes.
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE_POINT: u32 = 0x400;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
const ATTR_DEHYDRATED: u32 = ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS;

// Reparse tags.
const TAG_SYMLINK: u32 = 0xA000_000C;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
/// OneDrive and other cloud-files providers: 0x9000001A, 0x9000101A, ... 0x9000F01A.
const TAG_CLOUD_BASE: u32 = 0x9000_001A;
const TAG_CLOUD_MASK: u32 = 0xFFFF_0FFF;

// Byte offsets inside FILE_ID_EXTD_DIR_INFO.
const OFF_LAST_WRITE: usize = 24;
const OFF_END_OF_FILE: usize = 40;
const OFF_ALLOCATION: usize = 48;
const OFF_ATTRIBUTES: usize = 56;
const OFF_NAME_LEN: usize = 60;
const OFF_REPARSE_TAG: usize = 68;
const OFF_FILE_ID: usize = 72;
const OFF_NAME: usize = 88;

const BUF_BYTES: usize = 256 * 1024;
const FILETIME_UNIX_DIFF_SECS: i64 = 11_644_473_600;

thread_local! {
    /// 8-byte aligned scratch buffer, reused across directories on each worker thread.
    static BUF: RefCell<Vec<u64>> = RefCell::new(vec![0u64; BUF_BYTES / 8]);
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateFileW and is closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// `\\?\`-prefixed, NUL-terminated UTF-16 so paths over MAX_PATH work.
fn wide_extended(path: &Path) -> Vec<u16> {
    let p: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let mut out = Vec::with_capacity(p.len() + 8);
    if p.starts_with(&verbatim) {
        out.extend_from_slice(&p);
    } else if p.starts_with(&[u16::from(b'\\'), u16::from(b'\\')]) {
        out.extend(r"\\?\UNC\".encode_utf16());
        out.extend_from_slice(&p[2..]);
    } else {
        out.extend_from_slice(&verbatim);
        out.extend_from_slice(&p);
    }
    out.push(0);
    out
}

fn open_dir(path: &Path, access: u32) -> Result<Handle, u32> {
    let wide = wide_extended(path);
    // SAFETY: `wide` is NUL-terminated and outlives the call; null security attributes and
    // template are permitted.
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: trivially safe thread-local error read.
        Err(unsafe { GetLastError() })
    } else {
        Ok(Handle(h))
    }
}

pub fn volume_id(root: &Path) -> Option<String> {
    let h = open_dir(root, FILE_READ_ATTRIBUTES).ok()?;
    let mut buf = [0u64; 3]; // FILE_ID_INFO: u64 volume serial + 16-byte file id
                             // SAFETY: buffer is 24 bytes, matching FILE_ID_INFO.
    let ok = unsafe {
        GetFileInformationByHandleEx(h.0, FILE_ID_INFO, buf.as_mut_ptr() as *mut c_void, 24)
    };
    (ok != 0).then(|| format!("{:016X}", buf[0]))
}

fn classify_open(e: u32) -> DirErr {
    match e {
        ERROR_ACCESS_DENIED => DirErr::Denied,
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => DirErr::Locked,
        _ => DirErr::Other,
    }
}

fn classify_query(e: u32) -> DirErr {
    match e {
        ERROR_INVALID_FUNCTION
        | ERROR_NOT_SUPPORTED
        | ERROR_INVALID_PARAMETER
        | ERROR_CALL_NOT_IMPLEMENTED
        | ERROR_INVALID_LEVEL => DirErr::Unsupported,
        other => classify_open(other),
    }
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn i64_at(b: &[u8], off: usize) -> i64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    i64::from_le_bytes(a)
}

fn parse_entry(b: &[u8], off: usize, name_len: usize) -> Option<RawEntry> {
    let name_bytes = &b[off + OFF_NAME..off + OFF_NAME + name_len];
    let wide: Vec<u16> = name_bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    if wide == [0x2e] || wide == [0x2e, 0x2e] {
        return None;
    }
    let os = OsString::from_wide(&wide);
    let mut flags = 0u16;
    if os.to_str().is_none() {
        flags |= FLAG_LOSSY;
    }

    let attrs = u32_at(b, off + OFF_ATTRIBUTES);
    let tag = u32_at(b, off + OFF_REPARSE_TAG);
    let logical = i64_at(b, off + OFF_END_OF_FILE).max(0) as u64;
    let allocated = i64_at(b, off + OFF_ALLOCATION).max(0) as u64;
    let filetime = i64_at(b, off + OFF_LAST_WRITE);
    let mtime =
        (filetime / 10_000_000 - FILETIME_UNIX_DIFF_SECS).clamp(0, i64::from(u32::MAX)) as u32;

    let dehydrated = attrs & ATTR_DEHYDRATED != 0;
    let cloud_tag = tag & TAG_CLOUD_MASK == TAG_CLOUD_BASE;
    let is_reparse = attrs & ATTR_REPARSE_POINT != 0;
    let is_link = is_reparse && !cloud_tag && !dehydrated;
    let kind = if is_link {
        match tag {
            TAG_SYMLINK => Kind::Symlink,
            TAG_MOUNT_POINT => Kind::Junction,
            _ => Kind::Reparse,
        }
    } else if attrs & ATTR_DIRECTORY != 0 {
        Kind::Dir
    } else {
        Kind::File
    };
    // A directory that is not populated yet would make Windows fetch its listing from the
    // cloud when entered, so it is flagged here and the walker does not descend into it.
    if dehydrated {
        flags |= FLAG_CLOUD;
    }

    let mut e = RawEntry::new(os.to_string_lossy().into_owned(), kind, allocated, logical);
    e.flags = flags;
    e.mtime = mtime;
    if kind == Kind::File {
        let mut id = [0u8; 16];
        id.copy_from_slice(&b[off + OFF_FILE_ID..off + OFF_FILE_ID + 16]);
        e.file_id = Some(u128::from_le_bytes(id));
    }
    Some(e)
}

pub fn read_dir(path: &Path, out: &mut Vec<RawEntry>) -> Result<(), DirErr> {
    let h = open_dir(path, FILE_LIST_DIRECTORY).map_err(classify_open)?;
    BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let cap = buf.len() * 8;
        let mut class = FILE_ID_EXTD_DIRECTORY_RESTART_INFO;
        loop {
            // SAFETY: `buf` is 8-byte aligned and `cap` bytes long; the handle is a live
            // directory handle opened with FILE_LIST_DIRECTORY.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    h.0,
                    class,
                    buf.as_mut_ptr() as *mut c_void,
                    cap as u32,
                )
            };
            class = FILE_ID_EXTD_DIRECTORY_INFO;
            if ok == 0 {
                // SAFETY: trivially safe thread-local error read.
                let e = unsafe { GetLastError() };
                return if e == ERROR_NO_MORE_FILES {
                    Ok(())
                } else {
                    Err(classify_query(e))
                };
            }
            // SAFETY: reinterpreting initialized u64 storage as bytes; same allocation and length.
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, cap) };
            let mut off = 0usize;
            loop {
                if off + OFF_NAME > cap {
                    break;
                }
                let name_len = u32_at(bytes, off + OFF_NAME_LEN) as usize;
                if off + OFF_NAME + name_len > cap {
                    break;
                }
                if let Some(e) = parse_entry(bytes, off, name_len) {
                    out.push(e);
                }
                let next = u32_at(bytes, off) as usize;
                if next == 0 {
                    break;
                }
                off += next;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_paths_get_verbatim_prefix() {
        let w = wide_extended(Path::new(r"C:\a"));
        assert_eq!(String::from_utf16_lossy(&w[..w.len() - 1]), r"\\?\C:\a");
        let u = wide_extended(Path::new(r"\\srv\share\a"));
        assert_eq!(
            String::from_utf16_lossy(&u[..u.len() - 1]),
            r"\\?\UNC\srv\share\a"
        );
        let v = wide_extended(Path::new(r"\\?\C:\a"));
        assert_eq!(String::from_utf16_lossy(&v[..v.len() - 1]), r"\\?\C:\a");
    }

    #[test]
    fn cloud_tag_mask_matches_provider_range() {
        assert_eq!(0x9000_001A & TAG_CLOUD_MASK, TAG_CLOUD_BASE);
        assert_eq!(0x9000_601A & TAG_CLOUD_MASK, TAG_CLOUD_BASE);
        assert_ne!(TAG_SYMLINK & TAG_CLOUD_MASK, TAG_CLOUD_BASE);
    }
}
