#![forbid(unsafe_code)]
//! dredge-core: index model, size accounting, snapshots and queries. No filesystem walking
//! and no platform-specific code lives here.

pub mod builder;
pub mod error;
pub mod model;
pub mod scanner;
pub mod util;

pub use builder::{ErrorCategory, IndexBuilder, MetaInit};
pub use error::Error;
pub use model::{Index, IndexMeta, Kind, RawEntry, ScanErrors};
pub use scanner::{ProgressSnapshot, ScanOptions, ScanProgress, Scanner};
