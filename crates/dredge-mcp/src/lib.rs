//! dredge-mcp: read-only MCP server over stdio. A thin adapter: every tool calls the same
//! `Engine` the CLI uses, so both frontends print identical JSON. Stdout carries only the
//! protocol; diagnostics go to stderr.

mod jobs;
mod server;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use dredge_core::{Engine, EngineConfig};
use rmcp::{transport::stdio, ServiceExt};

use crate::server::DredgeServer;

/// How often the idle check runs.
const IDLE_TICK: Duration = Duration::from_secs(10);

/// Serve MCP on stdin/stdout until the client disconnects. The loaded index is dropped after
/// `idle` without a query, so an idle server costs almost no memory.
pub fn serve(cfg: EngineConfig, idle: Duration) -> io::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = rt.block_on(async move {
        let engine = Arc::new(Engine::new(cfg));

        let idle_engine = engine.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(IDLE_TICK);
            loop {
                tick.tick().await;
                if idle_engine.unload_if_idle(idle) {
                    eprintln!("dredge-mcp: unloaded idle index");
                }
            }
        });

        let service = DredgeServer::new(engine)
            .serve(stdio())
            .await
            .map_err(io::Error::other)?;
        service.waiting().await.map_err(io::Error::other)?;
        Ok(())
    });
    // A scan still running in the background must not keep the process alive after disconnect.
    rt.shutdown_background();
    result
}
