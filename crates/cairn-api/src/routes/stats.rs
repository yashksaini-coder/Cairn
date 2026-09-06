use axum::{extract::State, Json};
use serde_json::json;

use crate::{
    db,
    error::ApiResult,
    state::{now, AppState},
};

pub async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "cairn-api", "version": env!("CARGO_PKG_VERSION") }))
}

pub async fn stats(State(s): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let now = now();
    let stats = db::stats(&s.pool, now).await?;
    // Surfaced so the dashboard can say "the index is N seconds behind"
    // rather than silently showing stale numbers (KPI P4).
    let index_lag_seconds = stats.last_indexed_at.map(|t| now - t);

    Ok(Json(json!({
        "stats": stats,
        "program_id": s.cfg.program_id.to_string(),
        "cluster_rpc": s.cfg.rpc_url,
        "transcription_enabled": s.transcriber.enabled(),
        "index_lag_seconds": index_lag_seconds,
        "as_of": now,
    })))
}
