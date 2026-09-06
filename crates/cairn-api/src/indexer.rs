//! Chain -> read model.
//!
//! §8.5 specified `programSubscribe` with reconnect backoff and a full resync
//! after a 60-second disconnect. This polls `getProgramAccounts` instead.
//!
//! The trade is deliberate. Polling has one code path where the websocket
//! design has four (backfill, tail, reconnect, resync), and three of those
//! only ever run when something has already gone wrong -- which is to say
//! they are the three least likely to have been tested by ship date. A 3s
//! poll meets KPI P4 (<5s read consistency) with a loop body that either
//! works on the first tick or is obviously broken.
//!
//! ponytail: `getProgramAccounts` returns every escrow on every tick. Fine at
//! demo scale, and it degrades gracefully (the RPC gets slower, nothing
//! breaks). Past a few thousand escrows, switch to `programSubscribe` with
//! this poll retained as the reconciliation path.

use crate::{db, state::AppState};
use anyhow::Result;
use cairn_core::Escrow;

pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(state.cfg.index_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        match sync_once(&state).await {
            Ok(n) => tracing::debug!(escrows = n, "indexed"),
            // A failed tick is not fatal: devnet RPC is flaky (§14) and the
            // next tick is three seconds away. Serving slightly stale rows
            // beats taking the API down over someone else's outage.
            Err(e) => tracing::warn!(error = ?e, "index tick failed; retrying next tick"),
        }
    }
}

pub async fn sync_once(state: &AppState) -> Result<usize> {
    let accounts = state.rpc.escrow_accounts(&state.cfg.program_id).await?;
    let now = crate::state::now();
    let mut written = 0usize;

    for account in &accounts {
        match Escrow::try_decode(&account.data) {
            Ok(escrow) => {
                db::upsert_escrow(&state.pool, &account.pubkey, &escrow, now).await?;
                written += 1;
            }
            // The discriminator filter should make this unreachable. If it
            // fires anyway, the program has been redeployed with a changed
            // layout and the indexer is now reading a format it predates --
            // worth a loud log, not worth stopping the whole sync.
            Err(e) => tracing::warn!(
                pubkey = %account.pubkey,
                error = %e,
                "skipping an account this build cannot decode"
            ),
        }
    }

    Ok(written)
}
