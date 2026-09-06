use axum::{
    extract::{Path, Query, State},
    Json,
};
use cairn_core::state::EscrowState;
use serde::Deserialize;
use serde_json::json;

use crate::{
    db::{self, NeedRow},
    error::{ApiError, ApiResult},
    state::{now, AppState},
    views::{EscrowDetail, EscrowList, EscrowView, RegisterNeed},
};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    donor: Option<String>,
    recipient: Option<String>,
    state: Option<String>,
    expired: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

pub async fn list(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<EscrowList>> {
    let state = match q.state.as_deref() {
        None => None,
        Some(raw) => Some(
            raw.parse::<EscrowState>()
                .map_err(|_| ApiError::BadRequest(format!("unknown state {raw:?}")))?,
        ),
    };

    let filter = db::EscrowFilter {
        donor: q.donor,
        recipient: q.recipient,
        state,
        expired: q.expired,
        // Capped so a client cannot ask for the whole table. KPI P5 budgets
        // 2s for a cold load of 50.
        limit: q.limit.unwrap_or(50).clamp(1, 200),
        offset: q.offset.unwrap_or(0).max(0),
    };

    let now = now();
    let rows = db::list_escrows(&s.pool, &filter, now).await?;

    // One round trip for the descriptions rather than one per row.
    let keys: Vec<String> = rows.iter().map(|r| r.pubkey.clone()).collect();
    let mut needs = db::needs_for(&s.pool, &keys).await?;

    let items: Vec<EscrowView> = rows
        .into_iter()
        .map(|escrow| {
            let need =
                needs.iter().position(|n| n.escrow == escrow.pubkey).map(|i| needs.swap_remove(i));
            EscrowView { expired: escrow.is_expired(now), escrow, need }
        })
        .collect();

    Ok(Json(EscrowList { escrows: items, limit: filter.limit, offset: filter.offset }))
}

pub async fn get_one(
    State(s): State<AppState>,
    Path(pubkey): Path<String>,
) -> ApiResult<Json<EscrowDetail>> {
    let now = now();
    let escrow = db::get_escrow(&s.pool, &pubkey)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no escrow indexed at {pubkey}")))?;

    let need = db::get_need(&s.pool, &pubkey).await?;
    let receipt = match &escrow.receipt_hash {
        Some(h) => db::get_receipt(&s.pool, h).await?,
        None => None,
    };
    let audio_url = receipt.as_ref().map(|r| s.blobs.public_url(&r.audio_key));

    Ok(Json(EscrowDetail {
        escrow: EscrowView { expired: escrow.is_expired(now), escrow, need },
        receipt,
        audio_url,
    }))
}

/// Attach the human-readable need text to an escrow.
///
/// Unauthenticated on purpose, and safe anyway: the text is accepted only if
/// it hashes to the `need_hash` the donor already committed on chain. The
/// chain is the authority on what this escrow is for, and this endpoint can
/// do nothing but supply a preimage that matches it. Anyone can call it;
/// nobody can lie through it.
pub async fn register_need(
    State(s): State<AppState>,
    Json(body): Json<RegisterNeed>,
) -> ApiResult<Json<serde_json::Value>> {
    let escrow = db::get_escrow(&s.pool, &body.escrow).await?.ok_or_else(|| {
        ApiError::NotFound("that escrow is not indexed yet; wait for confirmation and retry".into())
    })?;

    let description = cairn_core::normalize::normalize_text(&body.description);
    let title = cairn_core::normalize::normalize_text(&body.title);
    if description.is_empty() {
        return Err(ApiError::BadRequest("description is empty after normalisation".into()));
    }

    let computed = hex::encode(cairn_core::need_hash_of_normalized(&description));
    if computed != escrow.need_hash {
        return Err(ApiError::BadRequest(format!(
            "description does not match the need hash committed on chain \
             (committed {}, this text hashes to {computed})",
            escrow.need_hash
        )));
    }

    db::upsert_need(
        &s.pool,
        &NeedRow {
            escrow: body.escrow,
            title,
            description,
            need_hash: computed,
            created_at: now(),
        },
    )
    .await?;

    Ok(Json(json!({ "ok": true })))
}
