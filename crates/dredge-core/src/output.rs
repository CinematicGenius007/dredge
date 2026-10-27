//! The JSON documents every frontend prints. One envelope shape, typed records per `kind`.
//! `schema_json()` derives the machine-readable schema from these same types.

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::Error;
use crate::model::ScanErrors;
use crate::scanner::ProgressSnapshot;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, JsonSchema)]
pub struct Envelope<R, S> {
    pub schema_version: u32,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotRef>,
    pub records: Vec<R>,
    pub summary: S,
}

impl<R, S> Envelope<R, S> {
    pub fn new(kind: &'static str, snapshot: Option<SnapshotRef>, records: Vec<R>, summary: S) -> Self {
        Envelope {
            schema_version: SCHEMA_VERSION,
            kind,
            snapshot,
            records,
            summary,
        }
    }
}

/// Placeholder record type for envelopes whose `records` is always empty.
#[derive(Serialize, JsonSchema)]
pub struct NoRecord {}

#[derive(Serialize, JsonSchema)]
pub struct ErrorCounts {
    pub denied: u64,
    pub locked: u64,
    pub other: u64,
}

#[derive(Serialize, JsonSchema)]
pub struct SnapshotRef {
    pub root: String,
    /// RFC 3339 UTC.
    pub taken_at: String,
    /// Seconds since the snapshot was taken: the staleness signal.
    pub age_secs: u64,
    /// Present only when the scan hit unreadable items, so totals may be low.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_errors: Option<ErrorCounts>,
}

/// Row for `browse`, `largest` and `detail`. Optional fields appear only when relevant.
#[derive(Serialize, JsonSchema)]
pub struct EntryRecord {
    pub name: String,
    pub path: String,
    /// `file`, `dir`, `symlink`, `junction` or `reparse`. Links are never followed.
    pub kind: &'static str,
    /// On-disk size; for directories the whole subtree. Hard-linked data is counted once.
    pub allocated_bytes: u64,
    pub human: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pct_of_parent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pct_of_scope: Option<f64>,
    /// `cloud_placeholder`, `denied`, `hardlink_alias`, `estimated`, `name_lossy`, `mount_point`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_links: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct TypeRecord {
    pub ext: String,
    pub files: u64,
    pub allocated_bytes: u64,
    pub human: String,
    pub pct_of_scope: f64,
}

#[derive(Serialize, JsonSchema)]
pub struct VolumeRecord {
    pub mount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub fs: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub used_pct: f64,
    pub used_human: String,
    pub total_human: String,
    /// Present only when a snapshot rooted exactly at this mount exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_age_secs: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
pub struct SnapshotRecord {
    pub root: String,
    pub taken_at: String,
    pub age_secs: u64,
    pub nodes: u64,
    pub allocated_bytes: u64,
    pub human: String,
    pub file_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_id: Option<String>,
    pub format_version: u32,
}

/// A configured root that agents mostly work in (`DREDGE_FOCUS_ROOTS`).
#[derive(Serialize, JsonSchema)]
pub struct FocusRoot {
    pub path: String,
    /// True when some snapshot covers this path.
    pub scanned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_age_secs: Option<u64>,
    /// Root of the covering snapshot when it is not the focus root itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_by: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct VolumesSummary {
    pub volumes: usize,
    pub data_dir: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub focus: Vec<FocusRoot>,
}

#[derive(Serialize, JsonSchema)]
pub struct SnapshotsSummary {
    pub snapshots: usize,
    pub data_dir: String,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ScanSummary {
    pub root: String,
    pub scanner: String,
    pub files: u64,
    pub dirs: u64,
    pub allocated_bytes: u64,
    pub human: String,
    pub logical_bytes: u64,
    pub elapsed_ms: u64,
    pub files_per_sec: u64,
    pub hard_link_groups: u64,
    pub reparse_points: u64,
    pub cloud_placeholders: u64,
    pub errors: ScanErrors,
    pub snapshot_path: String,
    pub snapshot_bytes: u64,
    /// Only when the root is a volume root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_used_bytes: Option<u64>,
    /// Volume used space the scan could not attribute (metadata, shadow copies, unreadable folders).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unaccounted_bytes: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
pub struct BrowseSummary {
    pub path: String,
    pub allocated_bytes: u64,
    pub human: String,
    pub entries: usize,
    pub shown: usize,
    pub truncated: bool,
    pub remaining_entries: usize,
    pub remaining_bytes: u64,
}

#[derive(Clone, Default, Serialize, JsonSchema)]
pub struct Filters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_bytes: Option<u64>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.ext.is_none() && self.min_bytes.is_none()
    }
}

#[derive(Serialize, JsonSchema)]
pub struct LargestSummary {
    pub path: String,
    #[serde(skip_serializing_if = "Filters::is_empty")]
    pub filters: Filters,
    pub matched_files: u64,
    pub matched_bytes: u64,
    pub shown: usize,
    pub truncated: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct TypesSummary {
    pub path: String,
    pub types: usize,
    pub shown: usize,
    pub truncated: bool,
    pub remaining_bytes: u64,
}

#[derive(Serialize, JsonSchema)]
pub struct DetailSummary {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub depth: u32,
}

/// MCP only: state of a background scan.
#[derive(Serialize, JsonSchema)]
pub struct JobSummary {
    pub job_id: String,
    /// `running`, `done` or `failed`.
    pub state: &'static str,
    /// Paths this job scans, in order.
    pub roots: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_root: Option<String>,
    /// Live counters for the root being scanned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ProgressSnapshot>,
    pub elapsed_ms: u64,
    /// One entry per finished root.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ScanSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl From<&Error> for ErrorBody {
    fn from(e: &Error) -> Self {
        ErrorBody {
            code: e.code(),
            message: e.to_string(),
            hint: e.hint(),
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub struct ErrorEnvelope {
    pub schema_version: u32,
    pub kind: &'static str,
    pub error: ErrorBody,
}

pub type VolumesOut = Envelope<VolumeRecord, VolumesSummary>;
pub type ScanOut = Envelope<NoRecord, ScanSummary>;
pub type SnapshotsOut = Envelope<SnapshotRecord, SnapshotsSummary>;
pub type BrowseOut = Envelope<EntryRecord, BrowseSummary>;
pub type LargestOut = Envelope<EntryRecord, LargestSummary>;
pub type TypesOut = Envelope<TypeRecord, TypesSummary>;
pub type DetailOut = Envelope<EntryRecord, DetailSummary>;
pub type JobOut = Envelope<NoRecord, JobSummary>;

/// Any document a command can produce.
pub enum Output {
    Volumes(VolumesOut),
    Scan(ScanOut),
    Snapshots(SnapshotsOut),
    Browse(BrowseOut),
    Largest(LargestOut),
    Types(TypesOut),
    Detail(DetailOut),
    Job(JobOut),
    Error(ErrorEnvelope),
}

fn to_json<T: Serialize>(v: &T, pretty: bool) -> String {
    let r = if pretty {
        serde_json::to_string_pretty(v)
    } else {
        serde_json::to_string(v)
    };
    r.unwrap_or_else(|e| {
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "kind": "error",
            "error": { "code": "internal", "message": format!("could not serialize output: {e}") }
        })
        .to_string()
    })
}

impl Output {
    pub fn from_error(e: &Error) -> Output {
        Output::Error(ErrorEnvelope {
            schema_version: SCHEMA_VERSION,
            kind: "error",
            error: ErrorBody::from(e),
        })
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Output::Error(_))
    }

    /// The one JSON document, compact unless `pretty`.
    pub fn render(&self, pretty: bool) -> String {
        match self {
            Output::Volumes(e) => to_json(e, pretty),
            Output::Scan(e) => to_json(e, pretty),
            Output::Snapshots(e) => to_json(e, pretty),
            Output::Browse(e) => to_json(e, pretty),
            Output::Largest(e) => to_json(e, pretty),
            Output::Types(e) => to_json(e, pretty),
            Output::Detail(e) => to_json(e, pretty),
            Output::Job(e) => to_json(e, pretty),
            Output::Error(e) => to_json(e, pretty),
        }
    }
}

/// Describes every document kind, one property per `kind`.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct SchemaDoc {
    volumes: VolumesOut,
    scan: ScanOut,
    snapshots: SnapshotsOut,
    browse: BrowseOut,
    largest: LargestOut,
    types: TypesOut,
    detail: DetailOut,
    job: JobOut,
    error: ErrorEnvelope,
}

pub fn schema_json(pretty: bool) -> String {
    to_json(&schemars::schema_for!(SchemaDoc), pretty)
}
