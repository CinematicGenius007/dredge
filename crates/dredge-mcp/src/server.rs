use std::sync::Arc;
use std::time::Duration;

use dredge_core::util::{absolutize, current_dir_string};
use dredge_core::{Engine, Error, Output, Query};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::Deserialize;

use crate::jobs::Jobs;

const INSTRUCTIONS: &str = "Disk space explorer over saved scan snapshots. Start with \
disk_volumes (free space, snapshot age, focus roots). If a path has no snapshot, run \
disk_scan and poll disk_job_status. Then drill down: disk_browse (children of a path, \
largest first), disk_largest (biggest files), disk_types (space by extension), disk_detail \
(one path). Results are small and sorted by size; raise `limit` only if needed. Sizes are \
allocated bytes on disk. Check `snapshot.age_secs` and rescan if the data is stale.";

#[derive(Deserialize, schemars::JsonSchema)]
struct ScanParams {
    /// Directory to scan. Omit to scan the configured focus roots.
    path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct JobParams {
    /// The `job_id` returned by disk_scan.
    job_id: String,
    /// Wait up to this many seconds (max 60) for the scan to finish before answering.
    wait_seconds: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct BrowseParams {
    /// Absolute path, or relative to the server's working directory.
    path: String,
    /// Rows to return. Default 10, max 1000.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct LargestParams {
    /// Absolute path, or relative to the server's working directory.
    path: String,
    /// Only files with this extension, e.g. "log".
    ext: Option<String>,
    /// Only files at least this many allocated bytes.
    min_bytes: Option<u64>,
    /// Rows to return. Default 10, max 1000.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct TypesParams {
    /// Absolute path, or relative to the server's working directory.
    path: String,
    /// Rows to return. Default 10, max 1000.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DetailParams {
    /// Absolute path, or relative to the server's working directory.
    path: String,
}

#[derive(Clone)]
pub struct DredgeServer {
    engine: Arc<Engine>,
    jobs: Arc<Jobs>,
}

fn reply(out: Output) -> Result<CallToolResult, McpError> {
    let blocks = vec![ContentBlock::text(out.render(false))];
    Ok(if out.is_error() {
        CallToolResult::error(blocks)
    } else {
        CallToolResult::success(blocks)
    })
}

#[tool_router]
impl DredgeServer {
    pub fn new(engine: Arc<Engine>) -> Self {
        DredgeServer {
            jobs: Arc::new(Jobs::new(engine.clone())),
            engine,
        }
    }

    /// Queries load snapshots from disk, so they run off the async threads.
    async fn query(&self, q: Query) -> Result<CallToolResult, McpError> {
        let engine = self.engine.clone();
        let out = tokio::task::spawn_blocking(move || engine.run(&q))
            .await
            .unwrap_or_else(|e| Output::from_error(&Error::Internal(e.to_string())));
        reply(out)
    }

    #[tool(
        description = "List volumes with total/used/free space, how old each snapshot is, and \
whether the focus roots are scanned. Call this first.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn disk_volumes(&self) -> Result<CallToolResult, McpError> {
        self.query(Query::Volumes).await
    }

    #[tool(
        description = "Scan a directory (or, with no path, the focus roots) and save a snapshot \
that the other tools query. Returns a job_id immediately; poll disk_job_status. Only writes \
dredge's own snapshot file; nothing else on disk is changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn disk_scan(
        &self,
        Parameters(p): Parameters<ScanParams>,
    ) -> Result<CallToolResult, McpError> {
        let roots = match p.path.filter(|s| !s.trim().is_empty()) {
            Some(path) => vec![absolutize(&path, &current_dir_string())],
            None => self.engine.focus_roots().to_vec(),
        };
        if roots.is_empty() {
            return reply(Output::from_error(&Error::InvalidArgument(
                "no path given and DREDGE_FOCUS_ROOTS is not set".into(),
            )));
        }
        reply(Output::Job(self.jobs.start(roots).summary()))
    }

    #[tool(
        description = "State and result of a scan started by disk_scan. Use wait_seconds to wait \
for it instead of polling.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn disk_job_status(
        &self,
        Parameters(p): Parameters<JobParams>,
    ) -> Result<CallToolResult, McpError> {
        let Some(job) = self.jobs.get(&p.job_id) else {
            return reply(Output::from_error(&Error::JobNotFound(p.job_id)));
        };
        job.wait(Duration::from_secs(p.wait_seconds.unwrap_or(0).min(60)))
            .await;
        reply(Output::Job(job.summary()))
    }

    #[tool(
        description = "Children of a directory, largest first, with each one's share of its \
parent. Use it to drill down from a volume or folder.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn disk_browse(
        &self,
        Parameters(p): Parameters<BrowseParams>,
    ) -> Result<CallToolResult, McpError> {
        self.query(Query::Browse {
            path: p.path,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        description = "Largest files anywhere under a path, optionally filtered by extension or \
minimum size.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn disk_largest(
        &self,
        Parameters(p): Parameters<LargestParams>,
    ) -> Result<CallToolResult, McpError> {
        self.query(Query::Largest {
            path: p.path,
            ext: p.ext,
            min_bytes: p.min_bytes,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        description = "Space used per file extension under a path, largest first.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn disk_types(
        &self,
        Parameters(p): Parameters<TypesParams>,
    ) -> Result<CallToolResult, McpError> {
        self.query(Query::Types {
            path: p.path,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        description = "Full details for one file or directory: sizes, counts, flags, modified time.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn disk_detail(
        &self,
        Parameters(p): Parameters<DetailParams>,
    ) -> Result<CallToolResult, McpError> {
        self.query(Query::Detail { path: p.path }).await
    }
}

#[tool_handler]
impl ServerHandler for DredgeServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("dredge", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
