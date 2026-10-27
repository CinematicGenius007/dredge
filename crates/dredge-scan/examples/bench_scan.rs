//! Scan a path and print files/sec and peak memory: `cargo run --release -p dredge-scan --example bench_scan -- <path>`
//! The scan result is discarded; this measures the scanner and index build only.

use std::path::PathBuf;
use std::time::Instant;

use dredge_core::{ScanOptions, ScanProgress, Scanner};
use dredge_scan::WalkScanner;

#[cfg(windows)]
fn peak_memory_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: zeroed POD struct with `cb` set, filled by the call for the current process.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            c.PeakWorkingSetSize as u64
        } else {
            0
        }
    }
}

#[cfg(unix)]
fn peak_memory_bytes() -> u64 {
    // SAFETY: zeroed POD struct filled by getrusage.
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
            return 0;
        }
        let v = ru.ru_maxrss as u64;
        // ru_maxrss is bytes on macOS, KiB elsewhere.
        if cfg!(target_os = "macos") {
            v
        } else {
            v * 1024
        }
    }
}

#[cfg(not(any(windows, unix)))]
fn peak_memory_bytes() -> u64 {
    0
}

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: bench_scan <path>");
        std::process::exit(2);
    };
    let progress = ScanProgress::default();
    let t = Instant::now();
    let index = WalkScanner
        .scan(&path, &ScanOptions::default(), &progress)
        .unwrap_or_else(|e| {
            eprintln!("scan failed: {e}");
            std::process::exit(1);
        });
    let secs = t.elapsed().as_secs_f64();
    let m = &index.meta;
    println!(
        "path={} nodes={} files={} dirs={} elapsed_s={:.2} files_per_sec={:.0} peak_mem_mib={:.1} errors={}",
        path.display(),
        m.node_count,
        m.files,
        m.dirs,
        secs,
        m.files as f64 / secs.max(1e-9),
        peak_memory_bytes() as f64 / (1024.0 * 1024.0),
        m.errors.total(),
    );
}
