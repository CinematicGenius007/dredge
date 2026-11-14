//! The single entry point both frontends call. It resolves the covering snapshot, runs one
//! query function, and wraps the result in the shared output envelope.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::categories::{self, Classes};
use crate::error::Error;
use crate::model::{Index, IndexMeta};
use crate::output::{
    CategoriesOut, DoctorOut, DoctorRecord, DoctorSummary, Envelope, ErrorCounts, FocusRoot,
    ForgetOut, ForgetRecord, ForgetSummary, InsightsOut, Output, ScanOut, ScanSummary,
    SnapshotRecord, SnapshotRef, SnapshotsOut, SnapshotsSummary, VolumeRecord, VolumesOut,
    VolumesSummary,
};
use crate::query::{self, DEFAULT_LIMIT, MAX_LIMIT};
use crate::scanner::{ScanOptions, ScanProgress, Scanner};
use crate::snapshot::FORMAT_VERSION;
use crate::store::{self, Dead, DeadFile, SnapshotFile, Store};
use crate::util::{
    absolutize, covers, current_dir_string, human_bytes, now_secs, parse_size, pct, rfc3339,
    same_path,
};
use crate::volumes;

#[derive(Clone, Default)]
pub struct EngineConfig {
    pub data_dir: PathBuf,
    /// Roots agents mostly work in; surfaced by `volumes` so they can see what is scanned.
    pub focus_roots: Vec<String>,
    /// Add every extra row field (CLI `--verbose`).
    pub verbose: bool,
    /// `DREDGE_MAX_DATA` as given: a size cap for the data directory, checked after each
    /// scan. `None` means no cap. Parsed (and rejected when invalid) by `scan` and `doctor`.
    pub max_data: Option<String>,
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
            max_data: std::env::var("DREDGE_MAX_DATA")
                .ok()
                .filter(|v| !v.trim().is_empty()),
        }
    }
}

/// One read-only question. Frontends build these from their own arguments.
pub enum Query {
    Volumes,
    Snapshots,
    Doctor,
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
    Categories {
        path: String,
        limit: Option<usize>,
    },
    Insights {
        path: String,
        limit: Option<usize>,
    },
    Detail {
        path: String,
    },
}

/// Which of dredge's own files `Engine::forget` removes.
pub enum Forget {
    /// The snapshot rooted at each path (exactly that root, not nested ones).
    Roots(Vec<String>),
    /// Shadowed, root-missing and unreadable snapshots, and stale temp files.
    Prune,
    /// Every snapshot and stale temp file.
    All,
}

struct Cached {
    file: PathBuf,
    taken_at: i64,
    index: Arc<Index>,
    /// Category of every node, computed on first use and kept with the index it describes.
    classes: Option<Arc<Classes>>,
    last_used: Instant,
}

pub struct Engine {
    store: Store,
    focus_roots: Vec<String>,
    verbose: bool,
    max_data: Option<String>,
    /// At most one loaded index; evicted on a different snapshot or after idling.
    cache: Mutex<Option<Cached>>,
}

struct Usage {
    total_bytes: u64,
    temps: usize,
    temp_bytes: u64,
    dead: Vec<DeadFile>,
}

fn age_secs(taken_at: i64) -> u64 {
    (now_secs() - taken_at).max(0) as u64
}

fn snapshot_ref(m: &IndexMeta) -> SnapshotRef {
    SnapshotRef {
        root: m.root.clone(),
        taken_at: rfc3339(m.taken_at),
        age_secs: age_secs(m.taken_at),
        scan_errors: (m.errors.total() > 0).then_some(ErrorCounts {
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
            max_data: cfg.max_data,
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
            Query::Doctor => Ok(Output::Doctor(self.doctor())),
            Query::Browse { path, limit } => {
                let limit = limit_or_default(*limit)?;
                let (idx, abs, sref) = self.open(path)?;
                let classes = self.verbose.then(|| self.classes(&idx));
                let (recs, sum) =
                    query::browse(&idx, &abs, limit, self.verbose, classes.as_deref())?;
                Ok(Output::Browse(Envelope::new(
                    "browse",
                    Some(sref),
                    recs,
                    sum,
                )))
            }
            Query::Largest {
                path,
                ext,
                min_bytes,
                limit,
            } => {
                let limit = limit_or_default(*limit)?;
                let (idx, abs, sref) = self.open(path)?;
                let classes = self.verbose.then(|| self.classes(&idx));
                let (recs, sum) = query::largest(
                    &idx,
                    &abs,
                    ext.as_deref(),
                    *min_bytes,
                    limit,
                    self.verbose,
                    classes.as_deref(),
                )?;
                Ok(Output::Largest(Envelope::new(
                    "largest",
                    Some(sref),
                    recs,
                    sum,
                )))
            }
            Query::Types { path, limit } => {
                let limit = limit_or_default(*limit)?;
                let (idx, abs, sref) = self.open(path)?;
                let (recs, sum) = query::types(&idx, &abs, limit)?;
                Ok(Output::Types(Envelope::new("types", Some(sref), recs, sum)))
            }
            Query::Categories { path, limit } => {
                Ok(Output::Categories(self.categories(path, *limit)?))
            }
            Query::Insights { path, limit } => Ok(Output::Insights(self.insights(path, *limit)?)),
            Query::Detail { path } => {
                let (idx, abs, sref) = self.open(path)?;
                let classes = self.classes(&idx);
                let (recs, sum) = query::detail(&idx, &abs, Some(&classes))?;
                Ok(Output::Detail(Envelope::new(
                    "detail",
                    Some(sref),
                    recs,
                    sum,
                )))
            }
        }
    }

    /// Space under `path` grouped by category, largest first (`Query::Categories`). The rows
    /// sum to the path's allocated bytes. The classification is one linear pass over the
    /// snapshot, computed once and cached with the loaded index.
    pub fn categories(&self, path: &str, limit: Option<usize>) -> Result<CategoriesOut, Error> {
        let limit = limit_or_default(limit)?;
        let (idx, abs, sref) = self.open(path)?;
        let node = query::resolve(&idx, &abs)?;
        let classes = self.classes(&idx);
        let (recs, sum) = categories::categories(&idx, &classes, node, limit);
        Ok(Envelope::new("categories", Some(sref), recs, sum))
    }

    /// Allocated bytes of each of `paths`, read from the snapshot that answers `root`. Sizes
    /// then agree with a `categories` or `insights` result for `root` even when a newer snapshot
    /// covers one of the paths, and no second snapshot is loaded. `None` for a path that
    /// snapshot does not hold. Library only (the TUI); no query kind and no output change.
    pub fn sizes_within(&self, root: &str, paths: &[String]) -> Result<Vec<Option<u64>>, Error> {
        let (idx, _, _) = self.open(root)?;
        Ok(paths
            .iter()
            .map(|p| idx.resolve(p).map(|i| idx.allocated(i)))
            .collect())
    }

    /// Cleanup hints under `path`, largest first (`Query::Insights`). Each carries a suggested
    /// command as text; dredge never runs it.
    pub fn insights(&self, path: &str, limit: Option<usize>) -> Result<InsightsOut, Error> {
        let limit = limit_or_default(limit)?;
        let (idx, abs, sref) = self.open(path)?;
        let node = query::resolve(&idx, &abs)?;
        let classes = self.classes(&idx);
        let own = absolutize(&self.store.dir().to_string_lossy(), &current_dir_string());
        let own_data = idx.resolve(&own);
        let (recs, sum) =
            categories::insights(&idx, &classes, node, limit, categories::host_os(), own_data);
        Ok(Envelope::new("insights", Some(sref), recs, sum))
    }

    /// The cached classification of `idx`, computed outside the lock on first use.
    fn classes(&self, idx: &Arc<Index>) -> Arc<Classes> {
        if let Some(c) = self
            .cache()
            .as_ref()
            .filter(|e| Arc::ptr_eq(&e.index, idx))
            .and_then(|e| e.classes.clone())
        {
            return c;
        }
        let classes = Arc::new(categories::classify(idx, categories::host_os()));
        if let Some(e) = self.cache().as_mut() {
            if Arc::ptr_eq(&e.index, idx) {
                e.classes = Some(classes.clone());
            }
        }
        classes
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
            classes: None,
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
        let cap = self.max_data_cap()?;
        // Wall clock for the whole operation (walk, index build, snapshot write), so the
        // reported time and files/sec match what the caller actually waited.
        let started = Instant::now();
        let abs = absolutize(path, &current_dir_string());
        let mut index = scanner.scan(Path::new(&abs), opts, progress)?;
        let ci = index.meta.case_insensitive;
        if let Some(v) = volumes::list()
            .into_iter()
            .find(|v| same_path(&v.mount, &abs, ci))
        {
            index.meta.volume_total_bytes = Some(v.total);
            index.meta.volume_used_bytes = Some(v.total.saturating_sub(v.free));
        }
        let (snapshot_path, snapshot_bytes) = self.store.save(&index)?;
        // A rescan within the same second would otherwise look identical to the cached one.
        self.drop_cached(&snapshot_path);
        let evicted = cap
            .map(|cap| self.evict_to(cap, &snapshot_path))
            .unwrap_or_default();
        let m = &index.meta;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let secs = (elapsed_ms as f64 / 1000.0).max(0.001);
        let summary = ScanSummary {
            root: m.root.clone(),
            scanner: m.scanner.clone(),
            files: m.files,
            dirs: m.dirs,
            allocated_bytes: m.allocated_total,
            human: human_bytes(m.allocated_total),
            logical_bytes: m.logical_total,
            elapsed_ms,
            files_per_sec: (m.files as f64 / secs) as u64,
            hard_link_groups: m.hard_link_groups,
            reparse_points: m.reparse_points,
            cloud_placeholders: m.cloud_placeholders,
            errors: m.errors.clone(),
            snapshot_path: snapshot_path.display().to_string(),
            snapshot_bytes,
            volume_used_bytes: m.volume_used_bytes,
            unaccounted_bytes: m
                .volume_used_bytes
                .map(|u| u.saturating_sub(m.allocated_total)),
            evicted,
        };
        Ok(Envelope::new(
            "scan",
            Some(snapshot_ref(m)),
            Vec::new(),
            summary,
        ))
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
                let cover = snaps
                    .iter()
                    .find(|s| covers(&s.meta.root, p, s.meta.case_insensitive));
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
        let list = self.store.list();
        let usage = self.usage(&list);
        let shadow = store::shadowed_by(&list);
        let records: Vec<SnapshotRecord> = list
            .into_iter()
            .zip(shadow)
            .map(|(s, shadowed_by)| SnapshotRecord {
                human: human_bytes(s.meta.allocated_total),
                age_secs: age_secs(s.meta.taken_at),
                taken_at: rfc3339(s.meta.taken_at),
                nodes: s.meta.node_count,
                allocated_bytes: s.meta.allocated_total,
                file_bytes: s.file_bytes,
                volume_id: s.meta.volume_id,
                format_version: FORMAT_VERSION,
                root_exists: store::root_exists(&s.meta.root),
                shadowed_by,
                root: s.meta.root,
            })
            .collect();
        let summary = SnapshotsSummary {
            snapshots: records.len(),
            data_dir: self.store.dir().display().to_string(),
            file_bytes: usage.total_bytes,
            human: human_bytes(usage.total_bytes),
            prunable: usage.dead.len(),
            prunable_bytes: usage.dead.iter().map(|d| d.file_bytes).sum(),
            temp_files: usage.temps,
            temp_bytes: usage.temp_bytes,
        };
        Envelope::new("snapshots", None, records, summary)
    }

    /// Bytes of every dredge file in the data directory, and what `--prune` would remove.
    fn usage(&self, list: &[SnapshotFile]) -> Usage {
        let dead = self.store.dead(list);
        let others: u64 = dead
            .iter()
            .filter(|d| d.root.is_none())
            .map(|d| d.file_bytes)
            .sum();
        let temps = dead.iter().filter(|d| d.why == Dead::TempFile);
        Usage {
            total_bytes: list.iter().map(|s| s.file_bytes).sum::<u64>() + others,
            temps: temps.clone().count(),
            temp_bytes: temps.map(|d| d.file_bytes).sum(),
            dead,
        }
    }

    /// `DREDGE_MAX_DATA` in bytes; an invalid value is an error so a typo never means "no cap".
    fn max_data_cap(&self) -> Result<Option<u64>, Error> {
        self.max_data
            .as_deref()
            .map(|v| {
                parse_size(v).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "DREDGE_MAX_DATA={v:?} is not a size; use bytes or a number with K, M, G or T, e.g. 2G"
                    ))
                })
            })
            .transpose()
    }

    fn drop_cached(&self, file: &Path) {
        let mut c = self.cache();
        if c.as_ref().is_some_and(|e| e.file == file) {
            *c = None;
        }
    }

    /// Remove dredge files until the data directory fits in `cap`: dead files first, then the
    /// oldest snapshots. `keep` (the snapshot just written) is never removed. Returns the roots
    /// removed, or the file path for a file without a readable root.
    fn evict_to(&self, cap: u64, keep: &Path) -> Vec<String> {
        let list = self.store.list();
        let usage = self.usage(&list);
        let mut total = usage.total_bytes;
        let mut order: Vec<(PathBuf, Option<String>, u64)> = usage
            .dead
            .into_iter()
            .map(|d| (d.path, d.root, d.file_bytes))
            .collect();
        for s in list.iter().rev() {
            if !order.iter().any(|(p, ..)| *p == s.path) {
                order.push((s.path.clone(), Some(s.meta.root.clone()), s.file_bytes));
            }
        }
        let mut evicted = Vec::new();
        for (path, root, bytes) in order {
            if total <= cap {
                break;
            }
            if path == keep || self.store.remove(&path).is_err() {
                continue;
            }
            self.drop_cached(&path);
            total = total.saturating_sub(bytes);
            evicted.push(root.unwrap_or_else(|| path.display().to_string()));
        }
        evicted
    }

    /// Remove dredge's own snapshot files (`dredge forget`). Nothing outside the data
    /// directory is touched, and nothing is removed on a dry run. A root with no snapshot
    /// fails the whole call before anything is removed.
    pub fn forget(&self, what: &Forget, dry_run: bool) -> Result<ForgetOut, Error> {
        let list = self.store.list();
        let shadow = store::shadowed_by(&list);
        let readable = |s: &SnapshotFile, reason: &'static str, by: Option<String>| ForgetRecord {
            root: Some(s.meta.root.clone()),
            file: s.path.display().to_string(),
            file_bytes: s.file_bytes,
            reason,
            shadowed_by: by,
            removed: false,
            error: None,
        };
        let from_dead = |d: DeadFile| ForgetRecord {
            root: d.root,
            file: d.path.display().to_string(),
            file_bytes: d.file_bytes,
            reason: d.why.reason(),
            shadowed_by: match d.why {
                Dead::Shadowed { by } => Some(by),
                _ => None,
            },
            removed: false,
            error: None,
        };
        let mut plan: Vec<(PathBuf, ForgetRecord)> = Vec::new();
        match what {
            Forget::Roots(roots) => {
                if roots.is_empty() {
                    return Err(Error::InvalidArgument(
                        "give a snapshot root, --prune or --all".into(),
                    ));
                }
                let cwd = current_dir_string();
                for r in roots {
                    require_path(r)?;
                    let abs = absolutize(r, &cwd);
                    let Some(s) = list
                        .iter()
                        .find(|s| same_path(&s.meta.root, &abs, s.meta.case_insensitive))
                    else {
                        let near = list
                            .iter()
                            .find(|s| covers(&s.meta.root, &abs, s.meta.case_insensitive))
                            .map(|s| format!(" (it is inside the snapshot of {})", s.meta.root))
                            .unwrap_or_default();
                        return Err(Error::InvalidArgument(format!(
                            "no snapshot is rooted at {abs}{near}; `dredge snapshots` lists the roots"
                        )));
                    };
                    if !plan.iter().any(|(p, _)| *p == s.path) {
                        plan.push((s.path.clone(), readable(s, "requested", None)));
                    }
                }
            }
            Forget::Prune => {
                for d in self.store.dead(&list) {
                    plan.push((d.path.clone(), from_dead(d)));
                }
            }
            Forget::All => {
                for (s, by) in list.iter().zip(shadow) {
                    plan.push((s.path.clone(), readable(s, "all", by)));
                }
                for d in self.store.dead(&list) {
                    if d.root.is_none() {
                        plan.push((d.path.clone(), from_dead(d)));
                    }
                }
            }
        }
        let mut removed = 0;
        let mut freed = 0;
        let mut records = Vec::with_capacity(plan.len());
        for (path, mut rec) in plan {
            if dry_run {
                removed += 1;
                freed += rec.file_bytes;
            } else {
                match self.store.remove(&path) {
                    Ok(()) => {
                        self.drop_cached(&path);
                        rec.removed = true;
                        removed += 1;
                        freed += rec.file_bytes;
                    }
                    Err(e) => rec.error = Some(e.to_string()),
                }
            }
            records.push(rec);
        }
        let remaining = if dry_run {
            let gone = records.iter().filter(|r| r.root.is_some()).count();
            list.len().saturating_sub(gone)
        } else {
            self.store.list().len()
        };
        let summary = ForgetSummary {
            removed,
            freed_bytes: freed,
            human: human_bytes(freed),
            dry_run,
            remaining,
            data_dir: self.store.dir().display().to_string(),
        };
        Ok(Envelope::new("forget", None, records, summary))
    }

    /// Read-only diagnostics for the snapshot store and configured focus roots.
    pub fn doctor(&self) -> DoctorOut {
        use std::fs;
        let dir = self.store.dir();
        let mut records = Vec::new();
        let exists = dir.is_dir();
        let writable = fs::metadata(dir).ok().is_some_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o222 != 0
            }
            #[cfg(windows)]
            {
                !m.permissions().readonly()
            }
            #[cfg(not(any(unix, windows)))]
            {
                !m.permissions().readonly()
            }
        });
        records.push(DoctorRecord {
            check: "data_dir".into(),
            status: if exists && writable { "ok" } else { "warn" },
            message: format!(
                "{}; {}",
                if exists { "exists" } else { "does not exist" },
                if writable {
                    "permissions allow writing"
                } else {
                    "write permission could not be confirmed"
                }
            ),
            hint: (!exists).then(|| "Run dredge scan to create the data directory.".into()),
        });

        let paths: Vec<_> = fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "dredge"))
            .collect();
        records.push(DoctorRecord {
            check: "snapshot_count".into(),
            status: "ok",
            message: format!("{} snapshot file(s) found", paths.len()),
            hint: None,
        });
        let mut unsupported = Vec::new();
        for p in &paths {
            let key = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let supported = crate::snapshot::FORMAT_VERSION;
            match crate::snapshot::read_version(p) {
                Ok(v) if v != supported => {
                    unsupported.push(key.clone());
                    records.push(DoctorRecord {
                        check: format!("snapshot_header:{key}"),
                        status: "warn",
                        message: format!(
                            "snapshot format version {v} is not supported (this build reads version {supported})"
                        ),
                        hint: Some("Rescan this root to replace the snapshot.".into()),
                    });
                }
                Ok(v) => match crate::snapshot::read_header(p) {
                    Ok(_) => records.push(DoctorRecord {
                        check: format!("snapshot_header:{key}"),
                        status: "ok",
                        message: format!("header parses; snapshot format version {v}"),
                        hint: None,
                    }),
                    Err(e) => records.push(DoctorRecord {
                        check: format!("snapshot_header:{key}"),
                        status: "warn",
                        message: format!("corrupt or unreadable snapshot: {e}"),
                        hint: Some("Rescan this root to replace the snapshot.".into()),
                    }),
                },
                Err(e) => records.push(DoctorRecord {
                    check: format!("snapshot_header:{key}"),
                    status: "warn",
                    message: format!("corrupt or unreadable snapshot: {e}"),
                    hint: Some("Rescan this root to replace the snapshot.".into()),
                }),
            }
        }
        records.push(DoctorRecord {
            check: "snapshot_format".into(),
            status: if unsupported.is_empty() { "ok" } else { "warn" },
            message: if unsupported.is_empty() {
                format!(
                    "supported snapshot format version {}",
                    crate::snapshot::FORMAT_VERSION
                )
            } else {
                format!(
                    "{} snapshot file(s) have an unsupported format version (supported: {})",
                    unsupported.len(),
                    crate::snapshot::FORMAT_VERSION
                )
            },
            hint: (!unsupported.is_empty()).then(|| "Rescan the affected roots.".into()),
        });

        let list = self.store.list();
        let usage = self.usage(&list);
        let cap = self.max_data_cap();
        let over = matches!(cap, Ok(Some(c)) if usage.total_bytes > c);
        records.push(DoctorRecord {
            check: "snapshot_storage".into(),
            status: if cap.is_err() || over { "warn" } else { "ok" },
            message: match &cap {
                Err(e) => e.to_string(),
                Ok(Some(c)) => format!(
                    "{} snapshot(s), {} on disk; DREDGE_MAX_DATA is {}",
                    list.len(),
                    human_bytes(usage.total_bytes),
                    human_bytes(*c)
                ),
                Ok(None) => format!(
                    "{} snapshot(s), {} on disk",
                    list.len(),
                    human_bytes(usage.total_bytes)
                ),
            },
            hint: over.then(|| {
                "The next scan evicts snapshots to fit; `dredge forget --prune` frees space now."
                    .into()
            }),
        });
        let dead_snaps: Vec<&DeadFile> = usage
            .dead
            .iter()
            .filter(|d| d.why != Dead::TempFile)
            .collect();
        records.push(DoctorRecord {
            check: "prunable".into(),
            status: if dead_snaps.is_empty() { "ok" } else { "warn" },
            message: if dead_snaps.is_empty() {
                "every snapshot can answer a query".into()
            } else {
                format!(
                    "{} snapshot file(s), {}, that no query uses (shadowed, root gone, or unreadable)",
                    dead_snaps.len(),
                    human_bytes(dead_snaps.iter().map(|d| d.file_bytes).sum())
                )
            },
            hint: (!dead_snaps.is_empty())
                .then(|| "dredge forget --prune --dry-run lists them; drop --dry-run to remove them.".into()),
        });
        records.push(DoctorRecord {
            check: "temp_files".into(),
            status: if usage.temps == 0 { "ok" } else { "warn" },
            message: if usage.temps == 0 {
                "no leftover temp files".into()
            } else {
                format!(
                    "{} temp file(s), {}, left by snapshot writes that never finished",
                    usage.temps,
                    human_bytes(usage.temp_bytes)
                )
            },
            hint: (usage.temps > 0).then(|| "dredge forget --prune".into()),
        });

        let data_path = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let volume = volumes::list()
            .into_iter()
            .filter(|v| data_path.starts_with(&v.mount))
            .max_by_key(|v| v.mount.len());
        if let Some(v) = volume {
            records.push(DoctorRecord {
                check: "disk_free".into(),
                status: "ok",
                message: format!("{} available on {}", human_bytes(v.free), v.mount),
                hint: None,
            });
        } else {
            records.push(DoctorRecord {
                check: "disk_free".into(),
                status: "warn",
                message: "free space for the data directory volume is unavailable".into(),
                hint: None,
            });
        }

        let snaps = list;
        for p in &self.focus_roots {
            if !Path::new(p).exists() {
                records.push(DoctorRecord {
                    check: format!("focus_root:{p}"),
                    status: "warn",
                    message: "configured focus root does not exist".into(),
                    hint: None,
                });
            }
            let cover = snaps
                .iter()
                .find(|s| covers(&s.meta.root, p, s.meta.case_insensitive));
            records.push(DoctorRecord {
                check: format!("focus_scan:{p}"),
                status: if cover.is_some() { "ok" } else { "warn" },
                message: cover
                    .map(|s| {
                        format!(
                            "scanned; snapshot age {} seconds",
                            age_secs(s.meta.taken_at)
                        )
                    })
                    .unwrap_or_else(|| "no snapshot covers this focus root".into()),
                hint: cover
                    .is_none()
                    .then(|| "Run dredge scan for this root.".into()),
            });
        }
        #[cfg(target_os = "macos")]
        if let Some(home) = dirs::home_dir() {
            let protected = home.join("Library/Safari");
            if let Err(e) = fs::read_dir(protected) {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    records.push(DoctorRecord { check: "full_disk_access".into(), status: "warn", message: "Full Disk Access may be limited".into(), hint: Some("Review macOS Privacy & Security settings if protected folders need scanning.".into()) });
                }
            }
        }
        let summary = DoctorSummary {
            checks: records.len(),
            data_dir: dir.display().to_string(),
        };
        Envelope::new("doctor", None, records, summary)
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
            ..Default::default()
        })
    }

    fn root() -> String {
        crate::util::lexical_clean(if cfg!(windows) {
            r"C:\fixture"
        } else {
            "/fixture"
        })
    }

    #[test]
    fn scan_then_query_through_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let r = root();
        let out = e
            .scan(
                &FakeScanner,
                &r,
                &ScanOptions::default(),
                &ScanProgress::default(),
            )
            .unwrap();
        assert_eq!(out.summary.files, 2);
        assert_eq!(out.summary.allocated_bytes, 4196);

        let json = e
            .run(&Query::Browse {
                path: r.clone(),
                limit: None,
            })
            .render(false);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "browse");
        assert_eq!(v["schema_version"], 4);
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
        let out = e.run(&Query::Browse {
            path: root(),
            limit: None,
        });
        assert!(out.is_error());
        let v: serde_json::Value = serde_json::from_str(&out.render(false)).unwrap();
        assert_eq!(v["kind"], "error");
        assert_eq!(v["error"]["code"], "no_snapshot");

        let bad = e.run(&Query::Browse {
            path: root(),
            limit: Some(0),
        });
        let v: serde_json::Value = serde_json::from_str(&bad.render(false)).unwrap();
        assert_eq!(v["error"]["code"], "invalid_argument");
    }

    #[test]
    fn newest_covering_snapshot_wins_for_subpaths() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let r = root();
        e.scan(
            &FakeScanner,
            &r,
            &ScanOptions::default(),
            &ScanProgress::default(),
        )
        .unwrap();
        let sub = format!("{r}{}sub", std::path::MAIN_SEPARATOR);
        let out = e.run(&Query::Detail { path: sub });
        let v: serde_json::Value = serde_json::from_str(&out.render(false)).unwrap();
        assert_eq!(v["kind"], "detail");
        assert_eq!(v["records"][0]["kind"], "dir");
    }

    #[test]
    fn sizes_within_reads_the_roots_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let r = root();
        scan_at(&e, &r).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let paths = vec![
            format!("{r}{sep}sub"),
            format!("{r}{sep}b.txt"),
            format!("{r}{sep}missing"),
        ];
        let sizes = e.sizes_within(&r, &paths).unwrap();
        let detail = |p: &str| match e.run(&Query::Detail { path: p.into() }) {
            Output::Detail(d) => d.records[0].allocated_bytes,
            _ => panic!("no detail"),
        };
        assert_eq!(sizes[0], Some(detail(&paths[0])));
        assert_eq!(sizes[1], Some(detail(&paths[1])));
        assert_eq!(sizes[2], None);
        assert!(matches!(
            e.sizes_within(&format!("{sep}elsewhere"), &paths),
            Err(Error::NoSnapshot { .. })
        ));
    }

    fn scan_at(e: &Engine, path: &str) -> Result<ScanOut, Error> {
        e.scan(
            &FakeScanner,
            path,
            &ScanOptions::default(),
            &ScanProgress::default(),
        )
    }

    #[test]
    fn cap_evicts_dead_snapshots_first_then_the_oldest_and_keeps_the_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        // Real directories: a missing root would also count as unused and change the order.
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::create_dir_all(tmp.path().join("other")).unwrap();
        let r = tree.to_string_lossy().into_owned();
        let sub = tree.join("sub").to_string_lossy().into_owned();
        let other = tmp.path().join("other").to_string_lossy().into_owned();
        scan_at(&engine(&dir), &sub).unwrap();
        scan_at(&engine(&dir), &other).unwrap();
        let capped = Engine::new(EngineConfig {
            data_dir: dir.clone(),
            max_data: Some("1".into()),
            ..Default::default()
        });
        let out = scan_at(&capped, &r).unwrap();
        assert_eq!(out.summary.evicted, [sub, other]);
        let left = capped.store().list();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].meta.root, r);

        let bad = Engine::new(EngineConfig {
            data_dir: dir,
            max_data: Some("2 gigs".into()),
            ..Default::default()
        });
        assert_eq!(scan_at(&bad, &r).err().unwrap().code(), "invalid_argument");
        let doc = bad.doctor();
        let storage = doc
            .records
            .iter()
            .find(|c| c.check == "snapshot_storage")
            .unwrap();
        assert_eq!(storage.status, "warn");
    }

    #[test]
    fn forget_drops_the_cached_index_and_dry_run_keeps_files() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path());
        let r = root();
        scan_at(&e, &r).unwrap();
        assert!(!e.run(&Query::Detail { path: r.clone() }).is_error());
        assert!(e.is_loaded());

        let dry = e.forget(&Forget::Roots(vec![r.clone()]), true).unwrap();
        assert_eq!(dry.summary.removed, 1);
        assert_eq!(dry.summary.remaining, 0);
        assert!(!dry.records[0].removed);
        assert_eq!(e.store().list().len(), 1);
        assert!(e.is_loaded());

        let done = e.forget(&Forget::Roots(vec![r.clone()]), false).unwrap();
        assert!(done.records[0].removed);
        assert_eq!(done.records[0].reason, "requested");
        assert!(!e.is_loaded());
        let v: serde_json::Value =
            serde_json::from_str(&e.run(&Query::Detail { path: r.clone() }).render(false)).unwrap();
        assert_eq!(v["error"]["code"], "no_snapshot");

        assert_eq!(
            e.forget(&Forget::Roots(vec![]), false)
                .err()
                .unwrap()
                .code(),
            "invalid_argument"
        );
        let none = e.forget(&Forget::Prune, false).unwrap();
        assert_eq!(none.summary.removed, 0);
    }

    #[test]
    fn snapshots_report_totals_and_shadowing() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(&dir.path().join("data"));
        // Real directories, so neither root counts as gone.
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        let r = tree.to_string_lossy().into_owned();
        let sub = tree.join("sub").to_string_lossy().into_owned();
        scan_at(&e, &sub).unwrap();
        scan_at(&e, &r).unwrap();
        let out = e.snapshots_out();
        assert!(out.records.iter().all(|x| x.root_exists));
        assert_eq!(out.summary.snapshots, 2);
        assert_eq!(out.summary.prunable, 1);
        assert_eq!(
            out.summary.file_bytes,
            out.records.iter().map(|r| r.file_bytes).sum::<u64>()
        );
        let shadowed: Vec<_> = out
            .records
            .iter()
            .filter(|x| x.shadowed_by.is_some())
            .collect();
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].root, sub);
        assert_eq!(shadowed[0].shadowed_by.as_deref(), Some(r.as_str()));
        let doc = e.doctor();
        let prunable = doc.records.iter().find(|c| c.check == "prunable").unwrap();
        assert_eq!(prunable.status, "warn");
    }
}
