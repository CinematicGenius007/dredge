//! Background scans. A scan can take minutes, so `disk_scan` starts a job and returns at once;
//! `disk_job_status` reads it (optionally waiting for the end).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dredge_core::output::{Envelope, ErrorBody, JobOut, JobSummary, ScanSummary};
use dredge_core::{Engine, Error, ScanOptions, ScanProgress};
use dredge_scan::WalkScanner;
use tokio::sync::watch;

/// Finished jobs stay queryable; older ones are forgotten.
const KEEP_JOBS: usize = 8;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Running,
    Done,
    Failed,
}

struct State {
    phase: Phase,
    current_root: Option<String>,
    progress: Option<Arc<ScanProgress>>,
    results: Vec<ScanSummary>,
    error: Option<ErrorBody>,
    elapsed_ms: Option<u64>,
}

pub struct Job {
    id: String,
    roots: Vec<String>,
    started: Instant,
    state: Mutex<State>,
    finished: watch::Sender<bool>,
}

impl Job {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn running(&self) -> bool {
        self.lock().phase == Phase::Running
    }

    pub fn summary(&self) -> JobOut {
        let st = self.lock();
        let running = st.phase == Phase::Running;
        let summary = JobSummary {
            job_id: self.id.clone(),
            state: match st.phase {
                Phase::Running => "running",
                Phase::Done => "done",
                Phase::Failed => "failed",
            },
            roots: self.roots.clone(),
            current_root: st.current_root.clone().filter(|_| running),
            progress: st.progress.as_ref().filter(|_| running).map(|p| p.snapshot()),
            elapsed_ms: st
                .elapsed_ms
                .unwrap_or_else(|| self.started.elapsed().as_millis() as u64),
            results: st.results.clone(),
            error: st.error.clone(),
        };
        Envelope::new("job", None, Vec::new(), summary)
    }

    /// Returns when the job has finished or `max` has passed, whichever is first.
    pub async fn wait(&self, max: Duration) {
        // Subscribe before checking, so a finish between the two cannot be missed.
        let mut rx = self.finished.subscribe();
        let done = *rx.borrow_and_update();
        if done || max.is_zero() {
            return;
        }
        let _ = tokio::time::timeout(max, rx.wait_for(|d| *d)).await;
    }

    fn finish(&self, first_error: Option<ErrorBody>) {
        {
            let mut st = self.lock();
            st.phase = if first_error.is_some() { Phase::Failed } else { Phase::Done };
            st.error = first_error;
            st.current_root = None;
            st.elapsed_ms = Some(self.started.elapsed().as_millis() as u64);
        }
        self.finished.send_replace(true);
    }
}

pub struct Jobs {
    engine: Arc<Engine>,
    list: Mutex<VecDeque<Arc<Job>>>,
    next: AtomicU64,
}

impl Jobs {
    pub fn new(engine: Arc<Engine>) -> Self {
        Jobs {
            engine,
            list: Mutex::new(VecDeque::new()),
            next: AtomicU64::new(1),
        }
    }

    fn list(&self) -> MutexGuard<'_, VecDeque<Arc<Job>>> {
        self.list.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        self.list().iter().find(|j| j.id == id).cloned()
    }

    /// Start scanning `roots` in order. Only one scan runs at a time (they are disk-bound and
    /// memory-heavy): while one is running, that job is returned instead of starting another.
    /// Must be called from inside the tokio runtime.
    pub fn start(&self, roots: Vec<String>) -> Arc<Job> {
        let mut list = self.list();
        if let Some(running) = list.iter().find(|j| j.running()) {
            return running.clone();
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let job = Arc::new(Job {
            id: format!("scan-{n}"),
            roots,
            started: Instant::now(),
            state: Mutex::new(State {
                phase: Phase::Running,
                current_root: None,
                progress: None,
                results: Vec::new(),
                error: None,
                elapsed_ms: None,
            }),
            finished: watch::channel(false).0,
        });
        list.push_back(job.clone());
        while list.len() > KEEP_JOBS {
            list.pop_front();
        }
        drop(list);

        let engine = self.engine.clone();
        let j = job.clone();
        tokio::task::spawn_blocking(move || run(&engine, &j));
        job
    }
}

fn run(engine: &Engine, job: &Job) {
    // A panic in a scanner must not leave the job "running" forever.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut first_error: Option<ErrorBody> = None;
        for root in &job.roots {
            let progress = Arc::new(ScanProgress::default());
            {
                let mut st = job.lock();
                st.current_root = Some(root.clone());
                st.progress = Some(progress.clone());
            }
            match engine.scan(&WalkScanner, root, &ScanOptions::default(), &progress) {
                Ok(out) => job.lock().results.push(out.summary),
                Err(e) => {
                    first_error.get_or_insert_with(|| ErrorBody::from(&e));
                }
            }
        }
        first_error
    }));
    let error = outcome.unwrap_or_else(|_| {
        Some(ErrorBody::from(&Error::Internal("scan thread panicked".into())))
    });
    job.finish(error);
}
