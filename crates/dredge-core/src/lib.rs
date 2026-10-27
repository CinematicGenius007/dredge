#![forbid(unsafe_code)]
//! dredge-core: index model, size accounting, snapshots and queries. No filesystem walking
//! and no platform-specific code lives here.

pub mod builder;
pub mod engine;
pub mod error;
pub mod model;
pub mod output;
pub mod query;
pub mod scanner;
pub mod snapshot;
pub mod store;
pub mod util;
pub mod volumes;

pub use builder::{ErrorCategory, IndexBuilder, MetaInit};
pub use engine::{Engine, EngineConfig, Query};
pub use error::Error;
pub use model::{Index, IndexMeta, Kind, RawEntry, ScanErrors};
pub use output::Output;
pub use scanner::{ProgressSnapshot, ScanOptions, ScanProgress, Scanner};
pub use store::{SnapshotFile, Store};
