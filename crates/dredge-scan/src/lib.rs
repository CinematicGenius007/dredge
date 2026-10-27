//! dredge-scan: scanner backends. All platform-specific code is isolated in `platform`.

mod platform;
mod walk;

pub use walk::WalkScanner;
