//! The snapshot data directory: one file per scanned root, newest-covering lookup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::Error;
use crate::model::{Index, IndexMeta};
use crate::snapshot;
use crate::util::{covers, fnv1a64, same_path};

const EXT: &str = "dredge";

/// A temp file this old is left over from a write that never finished. A live write keeps
/// touching its temp file and takes seconds, so an hour is far clear of one in progress.
pub const STALE_TEMP: Duration = Duration::from_secs(60 * 60);

/// A file in the data directory that `--prune` may remove, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dead {
    /// A newer snapshot covers this one's whole root, so lookup never returns it.
    Shadowed { by: String },
    /// The root is gone from a filesystem that is present (its parent still exists).
    RootMissing,
    /// Has the snapshot magic but a format version this build does not read.
    UnsupportedFormat(u32),
    /// Has the snapshot magic and a supported version, but the header does not parse.
    Corrupt,
    /// `*.dredge.<pid>.tmp` older than [`STALE_TEMP`].
    TempFile,
}

impl Dead {
    pub fn reason(&self) -> &'static str {
        match self {
            Dead::Shadowed { .. } => "shadowed",
            Dead::RootMissing => "root_missing",
            Dead::UnsupportedFormat(_) => "unsupported_format",
            Dead::Corrupt => "corrupt",
            Dead::TempFile => "temp_file",
        }
    }
}

/// One file `--prune` would remove. `root` is known only for readable snapshots.
#[derive(Clone, Debug)]
pub struct DeadFile {
    pub path: PathBuf,
    pub root: Option<String>,
    pub file_bytes: u64,
    pub taken_at: Option<i64>,
    pub why: Dead,
}

/// A `.dredge` or temp file that `list()` does not return.
#[derive(Clone, Debug)]
pub struct OtherFile {
    pub path: PathBuf,
    pub file_bytes: u64,
    pub why: Dead,
}

/// `<stem>.dredge.<digits>.tmp`, the name `snapshot::write` gives its temp file.
pub fn is_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".tmp") else {
        return false;
    };
    let Some((stem, pid)) = rest.rsplit_once('.') else {
        return false;
    };
    !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && stem.len() > EXT.len() + 1
        && stem.ends_with(&format!(".{EXT}"))
}

fn file_age(m: &fs::Metadata) -> Duration {
    m.modified()
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .unwrap_or(Duration::ZERO)
}

/// Whether the root is still there. Only a definite "not found" counts as gone: a permission
/// error says nothing about existence. `symlink_metadata` never follows a link and never
/// hydrates a cloud placeholder.
pub fn root_exists(root: &str) -> bool {
    match fs::symlink_metadata(root) {
        Ok(_) => true,
        Err(e) => e.kind() != io::ErrorKind::NotFound,
    }
}

/// A missing root counts as dead only when its parent folder still exists and the snapshot
/// is not of a whole volume. A drive that is not plugged in loses its mount path (and every
/// path under it), and its snapshot must survive `--prune`.
fn root_gone_for_good(s: &SnapshotFile) -> bool {
    if s.meta.volume_total_bytes.is_some() || root_exists(&s.meta.root) {
        return false;
    }
    Path::new(&s.meta.root)
        .parent()
        .is_some_and(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
}

/// For each snapshot in `list` (newest first, as `Store::list` returns), the root of the
/// first earlier snapshot that covers its root, which `find_covering` would always pick
/// instead.
pub fn shadowed_by(list: &[SnapshotFile]) -> Vec<Option<String>> {
    list.iter()
        .enumerate()
        .map(|(i, s)| {
            list[..i]
                .iter()
                .find(|t| {
                    covers(&t.meta.root, &s.meta.root, t.meta.case_insensitive)
                        && !same_path(&t.meta.root, &s.meta.root, t.meta.case_insensitive)
                })
                .map(|t| t.meta.root.clone())
        })
        .collect()
}

pub struct SnapshotFile {
    pub path: PathBuf,
    pub meta: IndexMeta,
    pub file_bytes: u64,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    /// `DREDGE_DATA_DIR`, else the OS local data directory.
    pub fn default_dir() -> PathBuf {
        if let Some(d) = std::env::var_os("DREDGE_DATA_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
        dirs::data_local_dir()
            .map(|d| d.join("dredge"))
            .unwrap_or_else(|| PathBuf::from(".dredge"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_for(&self, root: &str, case_insensitive: bool) -> PathBuf {
        let key = if case_insensitive {
            root.to_lowercase()
        } else {
            root.to_string()
        };
        let mut slug = String::new();
        for ch in root.chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch);
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug = slug.trim_matches('-');
        let slug = &slug[..slug.len().min(48)];
        self.dir
            .join(format!("{slug}-{:016x}.{EXT}", fnv1a64(key.as_bytes())))
    }

    /// Returns the snapshot path and its size in bytes. Stale temp files from writes that
    /// never finished are removed first.
    pub fn save(&self, index: &Index) -> Result<(PathBuf, u64), Error> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| Error::io(format!("creating data dir {}", self.dir.display()), e))?;
        for t in self
            .others()
            .into_iter()
            .filter(|o| o.why == Dead::TempFile)
        {
            let _ = self.remove(&t.path);
        }
        let path = self.path_for(&index.meta.root, index.meta.case_insensitive);
        let bytes = snapshot::write(&path, index)?;
        Ok((path, bytes))
    }

    /// All readable snapshots, newest first. Unreadable files are skipped, and so is anything
    /// that is not a regular file (a symlink in the data directory is never followed).
    pub fn list(&self) -> Vec<SnapshotFile> {
        let mut out: Vec<SnapshotFile> = self
            .entries()
            .into_iter()
            .filter(|(p, _)| p.extension().is_some_and(|x| x == EXT))
            .filter_map(|(path, m)| {
                let meta = snapshot::read_header(&path).ok()?;
                Some(SnapshotFile {
                    path,
                    meta,
                    file_bytes: m.len(),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.meta
                .taken_at
                .cmp(&a.meta.taken_at)
                .then(a.meta.root.cmp(&b.meta.root))
        });
        out
    }

    /// The newest snapshot whose root is `path` or an ancestor of it.
    pub fn find_covering(&self, path: &str) -> Option<SnapshotFile> {
        self.list()
            .into_iter()
            .find(|s| covers(&s.meta.root, path, s.meta.case_insensitive))
    }

    pub fn load(&self, path: &Path) -> Result<Index, Error> {
        snapshot::read(path)
    }

    /// Regular files directly in the data directory, with their metadata.
    fn entries(&self) -> Vec<(PathBuf, fs::Metadata)> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter_map(|e| {
                let m = fs::symlink_metadata(e.path()).ok()?;
                m.is_file().then(|| (e.path(), m))
            })
            .collect()
    }

    /// Files `list()` skips that `--prune` removes: snapshots this build cannot read (only
    /// with the dredge magic), and stale temp files. Young temp files are not returned.
    pub fn others(&self) -> Vec<OtherFile> {
        let mut out: Vec<OtherFile> = self
            .entries()
            .into_iter()
            .filter_map(|(path, m)| {
                let name = path.file_name()?.to_str()?.to_owned();
                let why = if is_temp_name(&name) {
                    (file_age(&m) >= STALE_TEMP).then_some(Dead::TempFile)?
                } else if path.extension().is_some_and(|x| x == EXT) && snapshot::has_magic(&path) {
                    match snapshot::read_version(&path) {
                        Ok(v) if v != snapshot::FORMAT_VERSION => Dead::UnsupportedFormat(v),
                        _ if snapshot::read_header(&path).is_ok() => return None,
                        _ => Dead::Corrupt,
                    }
                } else {
                    return None;
                };
                Some(OtherFile {
                    path,
                    file_bytes: m.len(),
                    why,
                })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Everything `dredge forget --prune` would remove now, given `list()`'s result.
    pub fn dead(&self, list: &[SnapshotFile]) -> Vec<DeadFile> {
        let mut out: Vec<DeadFile> = list
            .iter()
            .zip(shadowed_by(list))
            .filter_map(|(s, by)| {
                let why = match by {
                    Some(by) => Dead::Shadowed { by },
                    None if root_gone_for_good(s) => Dead::RootMissing,
                    None => return None,
                };
                Some(DeadFile {
                    path: s.path.clone(),
                    root: Some(s.meta.root.clone()),
                    file_bytes: s.file_bytes,
                    taken_at: Some(s.meta.taken_at),
                    why,
                })
            })
            .collect();
        out.extend(self.others().into_iter().map(|o| DeadFile {
            path: o.path,
            root: None,
            file_bytes: o.file_bytes,
            taken_at: None,
            why: o.why,
        }));
        out
    }

    /// Remove one of dredge's own files. Refuses anything that is not a regular file directly
    /// in the data directory carrying the snapshot magic, or a stale temp file. Links are never
    /// followed or removed.
    pub fn remove(&self, path: &Path) -> Result<(), Error> {
        let refuse = |why: &str| {
            Err(Error::InvalidArgument(format!(
                "refusing to remove {}: {why}",
                path.display()
            )))
        };
        if path.parent() != Some(self.dir.as_path()) {
            return refuse("not directly in the data directory");
        }
        let m = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => return Err(Error::io(format!("reading {}", path.display()), e)),
        };
        if !m.is_file() {
            return refuse("not a regular file");
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ours = if is_temp_name(name) {
            file_age(&m) >= STALE_TEMP
        } else {
            path.extension().is_some_and(|x| x == EXT) && snapshot::has_magic(path)
        };
        if !ours {
            return refuse("not a dredge snapshot or stale temp file");
        }
        fs::remove_file(path).map_err(|e| Error::io(format!("removing {}", path.display()), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{IndexBuilder, MetaInit};
    use crate::model::RawEntry;

    fn tiny(root: &str) -> Index {
        let mut b = IndexBuilder::new(root);
        b.add(0, RawEntry::file("f", 10));
        b.finish(MetaInit {
            scanner: "test".into(),
            volume_id: None,
            case_insensitive: false,
            elapsed_ms: 0,
        })
        .unwrap()
    }

    fn at(root: &str, taken_at: i64) -> Index {
        let mut i = tiny(root);
        i.meta.taken_at = taken_at;
        i
    }

    fn roots(v: &[SnapshotFile]) -> Vec<&str> {
        v.iter().map(|s| s.meta.root.as_str()).collect()
    }

    fn set_old(path: &Path) {
        let old = SystemTime::now() - STALE_TEMP - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn older_nested_snapshots_are_shadowed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        store.save(&at("/srv/app", 100)).unwrap(); // older, inside /srv: shadowed
        store.save(&at("/srv", 200)).unwrap();
        store.save(&at("/srv/logs", 300)).unwrap(); // newer than /srv: used
        store.save(&at("/srvx", 50)).unwrap(); // a sibling with a common prefix
        store.save(&at("/a", 400)).unwrap();
        store.save(&at("/a/b", 400)).unwrap(); // same second: lookup order decides
        let list = store.list();
        assert_eq!(
            roots(&list),
            ["/a", "/a/b", "/srv/logs", "/srv", "/srv/app", "/srvx"]
        );
        let by = shadowed_by(&list);
        assert_eq!(
            by,
            [
                None,
                Some("/a".into()),
                None,
                None,
                Some("/srv".into()),
                None
            ]
        );
        // What lookup does agrees: the shadowed root resolves to its shadow.
        assert_eq!(store.find_covering("/srv/app/x").unwrap().meta.root, "/srv");
        assert_eq!(store.find_covering("/a/b").unwrap().meta.root, "/a");
    }

    #[test]
    fn missing_root_is_dead_only_when_its_parent_is_still_there() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("tree");
        fs::create_dir_all(&base).unwrap();
        let store = Store::new(dir.path().join("data"));
        let gone = base.join("deleted-project");
        let unplugged = base.join("no-such-drive").join("photos");
        let here = base.to_string_lossy().into_owned();
        store.save(&at(&gone.to_string_lossy(), 10)).unwrap();
        store.save(&at(&unplugged.to_string_lossy(), 20)).unwrap();
        let mut vol = at(&dir.path().join("vol").to_string_lossy(), 30);
        vol.meta.volume_total_bytes = Some(1 << 40);
        store.save(&vol).unwrap();
        store
            .save(&at(&format!("{here}-sibling-exists"), 40))
            .unwrap();
        fs::create_dir(format!("{here}-sibling-exists")).unwrap();
        let list = store.list();
        let dead = store.dead(&list);
        assert_eq!(dead.len(), 1, "{dead:?}");
        assert_eq!(dead[0].why, Dead::RootMissing);
        assert_eq!(dead[0].root.as_deref(), Some(&*gone.to_string_lossy()));
        assert!(!root_exists(&unplugged.to_string_lossy()));
        assert!(root_exists(&here));
    }

    #[test]
    fn temp_names_match_what_write_produces() {
        assert!(is_temp_name("Users-me-0123456789abcdef.dredge.4242.tmp"));
        for name in [
            "x.dredge",
            "x.dredge.tmp",
            "x.dredge..tmp",
            "x.dredge.12a.tmp",
            ".dredge.1.tmp",
            "x.txt.1.tmp",
            "x.dredge.1.tmp.bak",
        ] {
            assert!(!is_temp_name(name), "{name}");
        }
    }

    #[test]
    fn stale_temp_files_are_cleaned_on_save_and_young_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let old = dir.path().join("srv-1.dredge.111.tmp");
        let young = dir.path().join("srv-1.dredge.222.tmp");
        fs::write(&old, b"partial").unwrap();
        fs::write(&young, b"in progress").unwrap();
        set_old(&old);
        let others = store.others();
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].path, old);
        assert!(
            store.remove(&young).is_err(),
            "a young temp file may be a live write"
        );
        store.save(&tiny("/srv")).unwrap();
        assert!(!old.exists());
        assert!(young.exists());
    }

    #[test]
    fn unreadable_snapshots_are_dead_but_foreign_files_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let (good, _) = store.save(&tiny("/srv")).unwrap();
        let bytes = fs::read(&good).unwrap();
        let mut future = bytes.clone();
        future[8..12].copy_from_slice(&99u32.to_le_bytes());
        fs::write(dir.path().join("future.dredge"), &future).unwrap();
        fs::write(dir.path().join("broken.dredge"), &bytes[..14]).unwrap();
        fs::write(dir.path().join("impostor.dredge"), b"hello").unwrap();
        fs::write(dir.path().join("notes.txt"), b"mine").unwrap();
        fs::create_dir(dir.path().join("sub.dredge")).unwrap();
        let whys: Vec<(String, Dead)> = store
            .others()
            .into_iter()
            .map(|o| (o.path.file_name().unwrap().to_string_lossy().into(), o.why))
            .collect();
        assert_eq!(
            whys,
            [
                ("broken.dredge".to_string(), Dead::Corrupt),
                ("future.dredge".to_string(), Dead::UnsupportedFormat(99)),
            ]
        );
        for name in ["impostor.dredge", "notes.txt", "sub.dredge"] {
            assert!(store.remove(&dir.path().join(name)).is_err(), "{name}");
            assert!(dir.path().join(name).exists());
        }
    }

    #[test]
    fn remove_refuses_paths_outside_the_data_dir_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let store = Store::new(data.clone());
        let (snap, _) = store.save(&tiny("/srv")).unwrap();
        // A real snapshot one level away is still refused.
        let outside = dir.path().join("copy.dredge");
        fs::copy(&snap, &outside).unwrap();
        assert!(store.remove(&outside).is_err());
        assert!(store.remove(&data.join("..").join("copy.dredge")).is_err());
        fs::create_dir(data.join("nested")).unwrap();
        let nested = data.join("nested").join("x.dredge");
        fs::copy(&snap, &nested).unwrap();
        assert!(store.remove(&nested).is_err());
        #[cfg(unix)]
        {
            let link = data.join("link.dredge");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(store.remove(&link).is_err());
            assert!(outside.exists());
            assert!(store.others().is_empty());
        }
        assert!(outside.exists() && nested.exists());
        store.remove(&snap).unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn save_list_and_find_covering() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("data"));
        assert!(store.list().is_empty());
        store.save(&tiny("/srv")).unwrap();
        store.save(&tiny("/srv/logs")).unwrap();
        assert_eq!(store.list().len(), 2);
        assert!(store.find_covering("/srv/logs/x").is_some());
        assert!(store.find_covering("/srv/code").is_some());
        assert!(store.find_covering("/other").is_none());
    }

    #[test]
    fn rescan_replaces_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let (p1, _) = store.save(&tiny("/srv")).unwrap();
        let (p2, _) = store.save(&tiny("/srv")).unwrap();
        assert_eq!(p1, p2);
        assert_eq!(store.list().len(), 1);
    }
}
