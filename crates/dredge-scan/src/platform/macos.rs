//! macOS backend: `getattrlistbulk(2)`. One system call returns name, type, sizes, inode, link
//! count, mtime and flags for a whole buffer of entries, so a directory costs a handful of
//! calls instead of one `lstat` per entry. Falls back to the `lstat` backend on file systems
//! that do not support the call.
//!
//! Directories marked dataless (`SF_DATALESS`, iCloud Drive and other File Provider
//! placeholders) are flagged as cloud placeholders and not entered: listing one would ask
//! the provider to download its contents.

use std::cell::RefCell;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use dredge_core::model::{FLAG_CLOUD, FLAG_LOSSY, FLAG_MOUNT};
use dredge_core::{Kind, RawEntry};

use super::generic::classify;
use super::DirErr;

// <sys/attr.h> and <sys/stat.h> values missing from `libc`.
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const SF_DATALESS: u32 = 0x4000_0000;
// <sys/vnode.h> `enum vtype` (VREG is 1; everything else is treated as a file).

const VDIR: u32 = 2;
const VLNK: u32 = 5;

const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;
const DIR: u32 = libc::ATTR_DIR_MOUNTSTATUS | libc::ATTR_DIR_ALLOCSIZE | libc::ATTR_DIR_DATALENGTH;
// <sys/attr.h> `ATTR_DIR_MOUNTSTATUS` bits.
const DIR_MNTSTATUS_MNTPOINT: u32 = 0x1;
const DIR_MNTSTATUS_TRIGGER: u32 = 0x2;
const FILE: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_TOTALSIZE | libc::ATTR_FILE_ALLOCSIZE;

/// Big enough for several hundred entries per call; reused per worker thread.
const BUF_BYTES: usize = 256 * 1024;

thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(vec![0; BUF_BYTES]);
}

struct Fd(libc::c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the descriptor was opened by `open_dir` and is closed exactly once.
        unsafe { libc::close(self.0) };
    }
}

fn errno() -> std::io::Error {
    std::io::Error::last_os_error()
}

fn open_dir(path: &Path) -> Result<Fd, std::io::Error> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains a NUL byte")
    })?;
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        Err(errno())
    } else {
        Ok(Fd(fd))
    }
}

/// A forward-only reader over one entry's packed attributes.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let v = self.b.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take::<4>().map(u32::from_ne_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take::<4>().map(i32::from_ne_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.take::<8>().map(i64::from_ne_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take::<8>().map(u64::from_ne_bytes)
    }
}

enum Parsed {
    Entry(RawEntry),
    /// `.`/`..` are never returned by the call, but a per-entry error can be.
    Error(i32),
}

/// Parses one packed entry. Attributes appear in bit order, common then dir then file, and
/// only the ones flagged in the returned set are present.
fn parse(b: &[u8], root_dev: Option<u64>) -> Option<Parsed> {
    let mut c = Cursor { b, at: 4 }; // skip the entry length
    let common = c.u32()?;
    let _vol = c.u32()?;
    let dir = c.u32()?;
    let file = c.u32()?;
    let _fork = c.u32()?;
    if common & ATTR_CMN_ERROR != 0 {
        let e = c.u32()?;
        if e != 0 {
            return Some(Parsed::Error(e as i32));
        }
    }
    let mut name: &[u8] = &[];
    if common & libc::ATTR_CMN_NAME != 0 {
        let base = c.at;
        let off = c.i32()?;
        let len = c.u32()? as usize;
        let start = usize::try_from(base as isize + off as isize).ok()?;
        // The length includes the trailing NUL.
        name = b.get(start..start + len.saturating_sub(1))?;
    }
    let mut dev = None;
    if common & libc::ATTR_CMN_DEVID != 0 {
        dev = Some(c.i32()? as u64); // same widening as `MetadataExt::dev`
    }
    let mut obj = 0;
    if common & libc::ATTR_CMN_OBJTYPE != 0 {
        obj = c.u32()?;
    }
    let mut mtime = 0i64;
    if common & libc::ATTR_CMN_MODTIME != 0 {
        mtime = c.i64()?;
        let _nsec = c.i64()?;
    }
    let mut st_flags = 0;
    if common & libc::ATTR_CMN_FLAGS != 0 {
        st_flags = c.u32()?;
    }
    let mut ino = None;
    if common & libc::ATTR_CMN_FILEID != 0 {
        ino = Some(c.u64()?);
    }
    let (mut allocated, mut logical, mut nlink) = (0u64, 0u64, None);
    let mut mount_status = 0;
    if dir & libc::ATTR_DIR_MOUNTSTATUS != 0 {
        mount_status = c.u32()?;
    }
    if dir & libc::ATTR_DIR_ALLOCSIZE != 0 {
        allocated = c.i64()?.max(0) as u64;
    }
    if dir & libc::ATTR_DIR_DATALENGTH != 0 {
        logical = c.i64()?.max(0) as u64;
    }
    if file & libc::ATTR_FILE_LINKCOUNT != 0 {
        nlink = Some(c.u32()?);
    }
    if file & libc::ATTR_FILE_TOTALSIZE != 0 {
        logical = c.i64()?.max(0) as u64;
    }
    if file & libc::ATTR_FILE_ALLOCSIZE != 0 {
        allocated = c.i64()?.max(0) as u64;
    }

    // Anything that is neither a directory nor a symlink (fifos, sockets, devices) counts as
    // a file, as in the lstat backend.
    let kind = match obj {
        VDIR => Kind::Dir,
        VLNK => Kind::Symlink,
        _ => Kind::File,
    };
    let os = OsStr::from_bytes(name);
    let mut flags = 0;
    if os.to_str().is_none() {
        flags |= FLAG_LOSSY;
    }
    // The device ID of a mount point is the covered directory's, not the mounted volume's,
    // so mounts are found by their mount status. Automount triggers count too: entering one
    // would mount it (for example a network share).
    let mounted = mount_status & (DIR_MNTSTATUS_MNTPOINT | DIR_MNTSTATUS_TRIGGER) != 0;
    let other_dev = root_dev.is_some() && dev.is_some() && root_dev != dev;
    if kind == Kind::Dir && root_dev.is_some() && (mounted || other_dev) {
        flags |= FLAG_MOUNT;
    }
    if st_flags & SF_DATALESS != 0 {
        flags |= FLAG_CLOUD;
    }
    let mut e = RawEntry::new(os.to_string_lossy().into_owned(), kind, allocated, logical);
    e.flags = flags;
    e.mtime = mtime.clamp(0, i64::from(u32::MAX)) as u32;
    if kind == Kind::File {
        e.nlink = nlink.map(|n| n.max(1));
        e.file_id = ino.map(u128::from);
    }
    Some(Parsed::Entry(e))
}

/// Entries pushed before an error are kept, like the other backends.
pub fn read_dir(path: &Path, root_dev: Option<u64>, out: &mut Vec<RawEntry>) -> Result<(), DirErr> {
    let fd = open_dir(path).map_err(|e| classify(&e))?;
    let mut al = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON,
        volattr: 0,
        dirattr: DIR,
        fileattr: FILE,
        forkattr: 0,
    };
    BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut result = Ok(());
        let mut first = true;
        loop {
            // SAFETY: `fd` is an open directory, `al` is a valid attrlist and `buf` is a
            // writable buffer of the length passed.
            let n = unsafe {
                libc::getattrlistbulk(
                    fd.0,
                    (&mut al as *mut libc::attrlist).cast(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                )
            };
            if n < 0 {
                let e = errno();
                let unsupported = matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL));
                if first && unsupported {
                    return super::unix::read_dir(path, root_dev, out);
                }
                return Err(classify(&e));
            }
            if n == 0 {
                return result;
            }
            first = false;
            let mut at = 0usize;
            for _ in 0..n {
                let Some(len) = buf
                    .get(at..at + 4)
                    .map(|b| u32::from_ne_bytes(b.try_into().unwrap()) as usize)
                else {
                    return Err(DirErr::Other);
                };
                let Some(entry) = buf.get(at..at + len) else {
                    return Err(DirErr::Other);
                };
                match parse(entry, root_dev) {
                    Some(Parsed::Entry(e)) => out.push(e),
                    Some(Parsed::Error(code)) => {
                        result = Err(classify(&std::io::Error::from_raw_os_error(code)));
                    }
                    None => return Err(DirErr::Other),
                }
                at += len;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    /// The bulk backend reports exactly what `lstat` reports, entry for entry.
    #[test]
    fn matches_the_lstat_backend() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("small"), b"x").unwrap();
        std::fs::write(p.join("big"), vec![7u8; 300_000]).unwrap();
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/inner"), b"y").unwrap();
        std::fs::hard_link(p.join("small"), p.join("small-link")).unwrap();
        std::os::unix::fs::symlink("big", p.join("ln")).unwrap();
        std::fs::write(p.join("caf\u{e9} \u{1f600}"), b"z").unwrap();
        let dev = Some(std::fs::metadata(p).unwrap().dev());

        let mut bulk = Vec::new();
        read_dir(p, dev, &mut bulk).unwrap();
        let mut stat = Vec::new();
        super::super::unix::read_dir(p, dev, &mut stat).unwrap();
        let key = |v: &mut Vec<RawEntry>| v.sort_by(|a, b| a.name.cmp(&b.name));
        key(&mut bulk);
        key(&mut stat);
        assert_eq!(bulk.len(), stat.len());
        for (a, b) in bulk.iter().zip(&stat) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind, "{}", a.name);
            assert_eq!(a.flags, b.flags, "{}", a.name);
            assert_eq!(a.mtime, b.mtime, "{}", a.name);
            if a.kind != Kind::Dir {
                assert_eq!(a.logical, b.logical, "{}", a.name);
                assert_eq!(a.allocated, b.allocated, "{}", a.name);
            }
            if a.kind == Kind::File {
                assert_eq!(a.file_id, b.file_id, "{}", a.name);
                assert_eq!(a.nlink, b.nlink, "{}", a.name);
            }
        }
    }

    /// A mount point reports the covered directory's device ID, so mounts must come from the
    /// mount status: `/dev` (devfs) is always mounted on macOS.
    #[test]
    fn mount_points_are_flagged() {
        let dev = Some(std::fs::metadata("/").unwrap().dev());
        let mut out = Vec::new();
        read_dir(Path::new("/"), dev, &mut out).unwrap();
        let d = out.iter().find(|e| e.name == "dev").expect("/dev listed");
        assert_ne!(d.flags & FLAG_MOUNT, 0);
        let users = out
            .iter()
            .find(|e| e.name == "Users")
            .expect("/Users listed");
        assert_eq!(users.flags & FLAG_MOUNT, 0, "a firmlink is not a mount");
    }

    #[test]
    fn missing_and_non_directories_error() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        let mut out = Vec::new();
        assert!(read_dir(&dir.path().join("nope"), None, &mut out).is_err());
        assert!(read_dir(&f, None, &mut out).is_err());
        assert!(out.is_empty());
    }

    #[test]
    fn large_directories_span_many_calls() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5000 {
            std::fs::write(
                dir.path().join(format!("file-with-a-longish-name-{i:05}")),
                b"",
            )
            .unwrap();
        }
        let mut out = Vec::new();
        read_dir(dir.path(), None, &mut out).unwrap();
        assert_eq!(out.len(), 5000);
    }
}
