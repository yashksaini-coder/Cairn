pub mod escrows;
pub mod receipts;
pub mod stats;
pub mod verify;

use crate::state::AppState;
use axum::{routing::get, routing::post, Router};

pub fn router(state: AppState) -> Router {
    let max_body = state.cfg.max_audio_bytes + 64 * 1024; // audio + form fields

    Router::new()
        .route("/healthz", get(stats::healthz))
        .route("/v1/stats", get(stats::stats))
        .route("/v1/escrows", get(escrows::list))
        .route("/v1/escrows/{pubkey}", get(escrows::get_one))
        .route("/v1/needs", post(escrows::register_need))
        .route("/v1/receipts/challenge", post(receipts::challenge))
        .route("/v1/receipts", post(receipts::upload))
        .route("/v1/receipts/{hash}", get(receipts::get_one))
        .route("/v1/blobs/{*key}", get(receipts::blob))
        .route("/v1/verify/{signature}", get(verify::verify))
        .layer(axum::extract::DefaultBodyLimit::max(max_body))
        .with_state(state)
}
