use std::sync::Arc;

use crate::{auth::Challenges, blob::BlobStore, config::Config, rpc::Rpc, transcribe::Transcriber};
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub pool: SqlitePool,
    pub rpc: Rpc,
    pub blobs: BlobStore,
    pub transcriber: Arc<Transcriber>,
    pub challenges: Arc<Challenges>,
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
