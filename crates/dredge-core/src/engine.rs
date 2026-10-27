//! The single entry point both frontends call. It resolves the covering snapshot, runs one
//! query function, and wraps the result in the shared output envelope.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::model::{Index, IndexMeta};
use crate::output::{
    Envelope, ErrorCounts, FocusRoot, Output, ScanOut, ScanSummary, SnapshotRecord, SnapshotRef,
    SnapshotsOut, SnapshotsSummary, VolumeRecord, VolumesOut, VolumesSummary,
};
use crate::query::{self, DEFAULT_LIMIT, MAX_LIMIT};
use crate::scanner::{ScanOptions, ScanProgress, Scanner};
use crate::snapshot::FORMAT_VERSION;
use crate::store::Store;
use crate::util::{absolutize, covers, current_dir_string, human_bytes, now_secs, pct, rfc3339, same_path};
use crate::volumes;

pub struct EngineConfig {
    pub data_dir: PathBuf,
    /// Roots agents mostly work in; surfaced by `volumes` so they can see what is scanned.
    pub focus_roots: Vec<String>,
    /// Add every extra row field (CLI `--verbose`).
    pub verbose: bool,
}

impl EngineConfig {
    /// `DREDGE_FOCUS_ROOTS` is a platform path list (`;` on Windows, `:` elsewhere).
    pub fn from_env(data_dir: Option<PathBuf>, verbose: bool) -> Self {
        let cwd = current_dir_string();
        let mut focus_roots: Vec<String> = Vec::new();
        if let Some(v) = std::env::var_os("DREDGE_FOCUS_ROOTS") {
            for p in std::env::split_paths(&v) {
                let s = p.to_string_lossy();
                if s.trim().is_empty() {
                    continue;
                }
                let abs = absolutize(&s, &cwd);
                if !focus_roots.contains(&abs) {
                    focus_roots.push(abs);
                }
            }
        }
        EngineConfig {
            data_dir: data_dir.unwrap_or_else(Store::default_dir),
            focus_roots,
            verbose,
        }
    }
}

/// One read-only question. Frontends build these from their own arguments.
pub enum Query {
    Volumes,
    Snapshots,
    Browse {
        path: String,
        limit: Option<usize>,
    },
    Largest {
        path: String,
        ext: Option<String>,
        min_bytes: Option<u64>,
        limit: Option<usize>,
    },
    Types {
        path: String,
        limit: Option<usize>,
    },
    Detail {
        path: String,
    },
}

struct Cached {
    file: PathBuf,
    taken_at: i64,
    index: Arc<Index>,
    last_used: Instant,
}

pub struct Engine {
    store: Store,
    focus_roots: Vec<String>,
    verbose: bool,
    /// At most one loaded index; evicted on a different snapshot or after idling.
    cache: Mutex<Option<Cached>>,
}

fn age_secs(taken_at: i64) -> u64 {
    (now_secs() - taken_at).max(0) as u64
}

fn snapshot_ref(m: &IndexMeta) -> SnapshotRef {
    SnapshotRef {
        root: m.root.clone(),
        taken_at: rfc3339(m.taken_at),
        age_secs: age_secs(m.taken_at),
        scan_errors: (m.errors.total() > 0).then(|| ErrorCounts {
            denied: m.errors.denied,
            locked: m.errors.locked,
            other: m.errors.other,
        }),
    }
}

fn limit_or_default(limit: Option<usize>) -> Result<usize, Error> {
    match limit {
        None => Ok(DEFAULT_LIMIT),
        Some(0) => Err(Error::InvalidArgument("limit must be at least 1".into())),
        Some(n) => Ok(n.min(MAX_LIMIT)),
    }
}

fn require_path(path: &str) -> Result<(), Error> {
    if path.trim().is_empty() {
        Err(Error::InvalidArgument("path must not be empty".into()))
    } else {
        Ok(())
    }
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Engine {
            store: Store::new(cfg.data_dir),
            focus_roots: cfg.focus_roots,
            verbose: cfg.verbose,
            cache: Mutex::new(None),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn focus_roots(&self) -> &[String] {
        &self.focus_roots
    }

    fn cache(&self) -> MutexGuard<'_, Option<Cached>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Drop the loaded index if nothing has used it for `idle`. Returns true when one was dropped.
    pub fn unload_if_idle(&self, idle: Duration) -> bool {
        let mut c = self.cache();
        if c.as_ref().is_some_and(|e| e.last_used.elapsed() >= idle) {
            *c = None;
            return true;
        }
        false
    }

    pub fn is_loaded(&self) -> bool {
        self.cache().is_some()
    }

    /// Never fails: errors come back as an error document.
    pub fn run(&self, q: &Query) -> Output {
        self.try_run(q).unwrap_or_else(|e| Output::from_error(&e))
    }

    pub fn try_run(&self, q: &Query) -> Result<Output, Error> {
        match q {
            Query::Volumes => Ok(Output::Volumes(self.volumes_out())),
            Query::Snapshots => Ok(Output::Snapshots(self.snapshots_out())),
            Query::Browse { path, limit } => {
                let limit = limit_or_default(*limit)?;
                let (idx, abs, sref) = self.open(path)?;
                let (recs, sum) = query::browse(&idx, &abs, limit, self.verbose)?;
                Ok(Output::Browse(Envelope::new("browse", Some(sref), recs, sum)))
            }
            Query::Largest {
                path,
                ext,
                min_bytes,
                limit,
            } => {
                let limit = limit_or_default(*limit)?;
                let (idx, abs, sref) = self.open(path)?;
                let (recs, sum) =
                    query::largest(&idx, &abs, ext.as_deref(), *min_bytes, limit, self.verbose)?;
                Ok(Output::Largest(Envelope::new("largest", Some(sref), recs, sum)))
            }
            Query::Types { path, limit } => {
                let limit = limit_or_default(*limit)?;
                let (idx, abs, sref) = self.open(path)?;
                let (recs, sum) = query::types(&idx, &abs, limit)?;
                Ok(Output::Types(Envelope::new("types", Some(sref), recs, sum)))
            }
            Query::Detail { path } => {
                let (idx, abs, sref) = self.open(path)?;
                let (recs, sum) = query::detail(&idx, &abs)?;
                Ok(Output::Detail(Envelope::new("detail", Some(sref), recs, sum)))
            }
        }
    }

    /// Find the newest snapshot covering `path`, loading it unless it is already cached.
    fn open(&self, path: &str) -> Result<(Arc<Index>, String, SnapshotRef), Error> {
        require_path(path)?;
        let abs = absolutize(path, &current_dir_string());
        let snap = self
            .store
            .find_covering(&abs)
            .ok_or_else(|| Error::NoSnapshot { path: abs.clone() })?;
        let sref = snapshot_ref(&snap.meta);
        {
            let mut c = self.cache();
            if let Some(e) = c.as_mut() {
                if e.file == snap.path && e.taken_at == snap.meta.taken_at {
                    e.last_used = Instant::now();
                    return Ok((e.index.clone(), abs, sref));
                }
            }
        }
        // Loaded outside the lock so other queries are not blocked while a snapshot reads.
        let index = Arc::new(self.store.load(&snap.path)?);
        *self.cache() = Some(Cached {
            file: snap.path,
            taken_at: snap.meta.taken_at,
            index: index.clone(),
            last_used: Instant::now(),
        });
        Ok((index, abs, sref))
    }

    /// Scan `path` with `scanner`, save the snapshot, and describe the result.
    pub fn scan(
        &self,
        scanner: &dyn Scanner,
        path: &str,
        opts: &ScanOptions,
        progress: &ScanProgress,
    ) -> Result<ScanOut, Error> {
        require_path(path)?;
        let abs = absolutize(path, &current_dir_string());
        let mut index = scanner.scan(Path::new(&abs), opts, progress)?;
        let ci = index.meta.case_insensitive;
        if let Some(v) = volumes::list().into_iter().find(|v| same_path(&v.mount, &abs, ci)) {
            index.meta.volume_total_bytes = Some(v.total);
            index.meta.volume_used_bytes = Some(v.total.saturating_sub(v.free));
        }
        let (snapshot_path, snapshot_bytes) = self.store.save(&index)?;
        let m = &index.meta;
        let secs = (m.elapsed_ms as f64 / 1000.0).max(0.001);
        let summary = ScanSummary {
            root: m.root.clone(),
            scanner: m.scanner.clone(),
            files: m.files,
            dirs: m.dirs,
            allocated_bytes: m.allocated_total,
            human: human_bytes(m.allocated_total),
            logical_bytes: m.logical_total,
            elapsed_ms: m.elapsed_ms,
            files_per_sec: (m.files as f64 / secs) as u64,
            hard_link_groups: m.hard_link_groups,
            reparse_points: m.reparse_points,
            cloud_placeholders: m.cloud_placeholders,
            errors: m.errors.clone(),
            snapshot_path: snapshot_path.display().to_string(),
            snapshot_bytes,
            volume_used_bytes: m.volume_used_bytes,
            unaccounted_bytes: m.volume_used_bytes.map(|u| u.saturating_sub(m.allocated_total)),
        };
        Ok(Envelope::new("scan", Some(snapshot_ref(m)), Vec::new(), summary))
    }

    fn volumes_out(&self) -> VolumesOut {
        let snaps = self.store.list();
        let records: Vec<VolumeRecord> = volumes::list()
            .into_iter()
            .map(|v| {
                let used = v.total.saturating_sub(v.free);
                let age = snaps
                    .iter()
                    .find(|s| same_path(&s.meta.root, &v.mount, s.meta.case_insensitive))
                    .map(|s| age_secs(s.meta.taken_at));
                VolumeRecord {
                    label: (!v.label.is_empty()).then(|| v.label.clone()),
                    fs: v.fs.clone(),
                    total_bytes: v.total,
                    used_bytes: used,
                    free_bytes: v.free,
                    used_pct: pct(used, v.total),
                    used_human: human_bytes(used),
                    total_human: human_bytes(v.total),
                    snapshot_age_secs: age,
                    mount: v.mount,
                }
            })
            .collect();
        let focus = self
            .focus_roots
            .iter()
            .map(|p| {
                // `snaps` is newest first, so the first covering snapshot is the newest.
                let cover = snaps.iter().find(|s| covers(&s.meta.root, p, s.meta.case_insensitive));
                FocusRoot {
                    path: p.clone(),
                    scanned: cover.is_some(),
                    snapshot_age_secs: cover.map(|s| age_secs(s.meta.taken_at)),
                    covered_by: cover
                        .filter(|s| !same_path(&s.meta.root, p, s.meta.case_insensitive))
                        .map(|s| s.meta.root.clone()),
                }
            })
            .collect();
        let summary = VolumesSummary {
            volumes: records.len(),
            data_dir: self.store.dir().display().to_string(),
            focus,
        };
        Envelope::new("volumes", None, records, summary)
    }

    fn snapshots_out(&self) -> SnapshotsOut {
        let records: Vec<SnapshotRecord> = self
            .store
            .list()
            .into_iter()
            .map(|s| SnapshotRecord {
                human: human_bytes(s.meta.allocated_total),
                age_secs: age_secs(s.meta.taken_at),
                taken_at: rfc3339(s.meta.taken_at),
                nodes: s.meta.node_count,
                allocated_bytes: s.meta.allocated_total,
                file_bytes: s.file_bytes,
                volume_id: s.meta.volume_id,
                format_version: FORMAT_VERSION,
                root: s.meta.root,
            })
            .collect();
        let summary = SnapshotsSummary {
            snapshots: records.len(),
            data_dir: self.store.dir().display().to_string(),
        };
        Envelope::new("snapshots", None, records, summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{IndexBuilder, MetaInit};
    use crate::model::RawEntry;

    struct FakeScanner;

    impl Scanner for FakeScanner {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn scan(&self, root: &Path, _: &ScanOptions, _: &ScanProgress) -> Result<Index, Error> {
            let mut b = IndexBuilder::new(&root.to_string_lossy());
            let d = b.add(0, RawEntry::dir("sub"));
            b.add(d, RawEntry::file("a.bin", 4096));
            b.add(0, RawEntry::file("b.txt", 100));
            b.finish(MetaInit {
                scanner: "fake".into(),
                volume_id: None,
                case_insensitive: false,
                elapsed_ms: 10,
            })
        }
    }

    fn engine(dir: &Path) -> Engine {
        Engine::new(EngineConfig {
            data_dir: dir.to_path_buf(),
            focus_roots: vec![],
            verbose: false,
        })
    }

    fn root() -> String {
        crate::util::lexical_clean(if cfg!(windows) { r"C:\fixture" } else { "/fixture" })
    }

    #[test]
    fn scan_then_query_through_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let r = root();
        let out = e
            .scan(&FakeScanner, &r, &ScanOptions::default(), &ScanProgress::default())
            .unwrap();
        assert_eq!(out.summary.files, 2);
        assert_eq!(out.summary.allocated_bytes, 4196);

        let json = e.run(&Query::Browse { path: r.clone(), limit: None }).render(false);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "browse");
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["records"][0]["name"], "sub");
        assert!(v["snapshot"]["age_secs"].is_u64());
        assert!(e.is_loaded());
        assert!(e.unload_if_idle(Duration::ZERO));
        assert!(!e.is_loaded());
    }

    #[test]
    fn errors_are_documents_with_stable_codes() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let out = e.run(&Query::Browse { path: root(), limit: None });
        assert!(out.is_error());
        let v: serde_json::Value = serde_json::from_str(&out.render(false)).unwrap();
        assert_eq!(v["kind"], "error");
        assert_eq!(v["error"]["code"], "no_snapshot");

        let bad = e.run(&Query::Browse { path: root(), limit: Some(0) });
        let v: serde_json::Value = serde_json::from_str(&bad.render(false)).unwrap();
        assert_eq!(v["error"]["code"], "invalid_argument");
    }

    #[test]
    fn newest_covering_snapshot_wins_for_subpaths() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let r = root();
        e.scan(&FakeScanner, &r, &ScanOptions::default(), &ScanProgress::default())
            .unwrap();
        let sub = format!("{r}{}sub", std::path::MAIN_SEPARATOR);
        let out = e.run(&Query::Detail { path: sub });
        let v: serde_json::Value = serde_json::from_str(&out.render(false)).unwrap();
        assert_eq!(v["kind"], "detail");
        assert_eq!(v["records"][0]["kind"], "dir");
    }
}
