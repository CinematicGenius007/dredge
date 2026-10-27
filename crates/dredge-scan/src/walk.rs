use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Instant;

use dredge_core::model::{FLAG_CLOUD, FLAG_LOSSY, FLAG_MOUNT};
use dredge_core::util::{strip_verbatim, CASE_INSENSITIVE_DEFAULT};
use dredge_core::{
    Error, ErrorCategory, Index, IndexBuilder, Kind, MetaInit, ScanOptions, ScanProgress, Scanner,
};

use crate::platform::{self, DirErr, PlatformCtx};

/// Parallel directory walker: one rayon task per directory, results merged into one builder.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkScanner;

struct Ctx<'a> {
    builder: Mutex<IndexBuilder>,
    progress: &'a ScanProgress,
    pctx: PlatformCtx,
}

impl Scanner for WalkScanner {
    fn name(&self) -> &'static str {
        "walk"
    }

    fn scan(
        &self,
        root: &Path,
        opts: &ScanOptions,
        progress: &ScanProgress,
    ) -> Result<Index, Error> {
        let start = Instant::now();
        let md = std::fs::symlink_metadata(root)
            .map_err(|e| Error::ScanFailed(format!("{}: {e}", root.display())))?;
        if !md.is_dir() {
            return Err(Error::ScanFailed(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        let root_str = strip_verbatim(&root.to_string_lossy());
        let (volume_id, root_dev) = platform::volume_info(root);
        let ctx = Ctx {
            builder: Mutex::new(IndexBuilder::new(&root_str)),
            progress,
            pctx: PlatformCtx { root_dev },
        };
        let threads = opts
            .threads
            .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
            .unwrap_or(4)
            .max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(|e| Error::ScanFailed(format!("could not start thread pool: {e}")))?;
        pool.scope(|s| scan_dir(s, &ctx, root.to_path_buf(), 0));
        if progress.cancelled() {
            return Err(Error::ScanFailed("cancelled".into()));
        }
        let builder = ctx.builder.into_inner().unwrap_or_else(|p| p.into_inner());
        builder.finish(MetaInit {
            scanner: self.name().to_string(),
            volume_id,
            case_insensitive: CASE_INSENSITIVE_DEFAULT,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
}

fn category(e: DirErr) -> ErrorCategory {
    match e {
        DirErr::Denied => ErrorCategory::Denied,
        DirErr::Locked => ErrorCategory::Locked,
        DirErr::Other | DirErr::Unsupported => ErrorCategory::Other,
    }
}

fn scan_dir<'scope>(
    scope: &rayon::Scope<'scope>,
    ctx: &'scope Ctx<'scope>,
    path: PathBuf,
    id: u32,
) {
    if ctx.progress.cancelled() {
        return;
    }
    ctx.progress.dirs.fetch_add(1, Ordering::Relaxed);

    let mut entries = Vec::new();
    let res = platform::read_dir(&path, &ctx.pctx, &mut entries);

    let mut subdirs: Vec<(u32, String)> = Vec::new();
    let mut files = 0u64;
    let mut bytes = 0u64;
    {
        let mut b = ctx.builder.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(e) = res {
            b.record_error(category(e), &path.to_string_lossy());
            ctx.progress.errors.fetch_add(1, Ordering::Relaxed);
            if entries.is_empty() {
                b.mark_denied(id);
            }
        }
        for e in entries {
            let is_dir = e.kind == Kind::Dir;
            let enterable = is_dir && e.flags & (FLAG_MOUNT | FLAG_CLOUD) == 0;
            if enterable && e.flags & FLAG_LOSSY != 0 {
                // The lossy name cannot be turned back into a path, so the directory is not entered.
                b.record_error(ErrorCategory::Other, &path.join(&e.name).to_string_lossy());
                ctx.progress.errors.fetch_add(1, Ordering::Relaxed);
            }
            let enter = enterable && e.flags & FLAG_LOSSY == 0;
            bytes += e.allocated;
            if !is_dir {
                files += 1;
            }
            let name = enter.then(|| e.name.clone());
            let cid = b.add(id, e);
            if let Some(n) = name {
                subdirs.push((cid, n));
            }
        }
    }
    ctx.progress.files.fetch_add(files, Ordering::Relaxed);
    ctx.progress
        .allocated_bytes
        .fetch_add(bytes, Ordering::Relaxed);

    for (cid, name) in subdirs {
        let p = path.join(&name);
        scope.spawn(move |s| scan_dir(s, ctx, p, cid));
    }
}
