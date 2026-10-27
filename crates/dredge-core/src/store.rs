//! The snapshot data directory: one file per scanned root, newest-covering lookup.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::model::{Index, IndexMeta};
use crate::snapshot;
use crate::util::{covers, fnv1a64};

const EXT: &str = "dredge";

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

    /// Returns the snapshot path and its size in bytes.
    pub fn save(&self, index: &Index) -> Result<(PathBuf, u64), Error> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| Error::io(format!("creating data dir {}", self.dir.display()), e))?;
        let path = self.path_for(&index.meta.root, index.meta.case_insensitive);
        let bytes = snapshot::write(&path, index)?;
        Ok((path, bytes))
    }

    /// All readable snapshots, newest first. Unreadable files are skipped.
    pub fn list(&self) -> Vec<SnapshotFile> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<SnapshotFile> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == EXT))
            .filter_map(|path| {
                let meta = snapshot::read_header(&path).ok()?;
                let file_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                Some(SnapshotFile {
                    path,
                    meta,
                    file_bytes,
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
