//! Cairn's backend.
//!
//! It ingests audio, transcribes it, computes receipt hashes with the same
//! crate the program uses, indexes chain state, and independently verifies
//! releases.
//!
//! It holds no keys, signs nothing, and has no instruction path that moves
//! funds. If this process is fully compromised, an attacker can serve wrong
//! transcripts and delete blobs -- both detectable, because the hash that
//! matters is on chain -- and cannot take a lamport.

use std::sync::Arc;

use anyhow::{Context, Result};
use tower_http::trace::TraceLayer;

use cairn_api::{
    auth::Challenges, blob::BlobStore, config::Config, db, indexer, routes, rpc::Rpc,
    state::AppState, transcribe::Transcriber,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cairn_api=debug,tower_http=info,warn".into()),
        )
        .init();

    let cfg = Config::from_env().context("configuration")?;
    tracing::info!(program = %cfg.program_id, rpc = %cfg.rpc_url, "starting cairn-api");

    let pool = db::connect(&cfg.database_url).await.context("database")?;
    let blobs = BlobStore::new(&cfg.blob, &cfg.public_base_url).await.context("blob store")?;

    let state = AppState {
        rpc: Rpc::new(cfg.rpc_url.clone()),
        transcriber: Arc::new(Transcriber::new(cfg.elevenlabs_api_key.clone())),
        challenges: Arc::new(Challenges::new(cfg.challenge_ttl)),
        cfg: Arc::new(cfg),
        pool,
        blobs,
    };

    // The indexer runs beside the HTTP server rather than inside a request
    // path, sharing the pool. If it dies the API keeps serving the rows it
    // already has, which is the correct failure mode for a cache.
    tokio::spawn(indexer::run(state.clone()));

    // No CORS layer. Nothing calls this from a browser -- the client is a
    // terminal binary, and an API that only ever answers `cairn` and `curl`
    // has no origins to allow.
    let app = routes::router(state.clone()).layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(state.cfg.bind).await?;
    tracing::info!(addr = %state.cfg.bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}
