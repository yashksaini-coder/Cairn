//! The read model.
//!
//! Nothing here is authoritative. Every escrow row is a cache of an on-chain
//! account, and the `needs`/`receipts` tables hold artifacts whose integrity
//! is guaranteed by a hash that lives on chain, not by this database. Drop
//! the file and the project loses its speed, not its claims.

use anyhow::Result;
use cairn_core::{state::EscrowState, Escrow};
use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;
use sqlx::{sqlite::SqlitePoolOptions, FromRow, QueryBuilder, Row, SqlitePool};

pub async fn connect(url: &str) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new().max_connections(5).connect(url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct EscrowRow {
    pub pubkey: String,
    pub donor: String,
    pub recipient: String,
    pub amount: i64,
    pub need_hash: String,
    pub receipt_hash: Option<String>,
    pub deadline: i64,
    pub state: String,
    pub created_at: i64,
    pub released_at: Option<i64>,
    pub indexed_at: i64,
}

impl EscrowRow {
    /// Derived on read rather than written by a sweeper.
    ///
    /// §8.6 specified a worker that flags expiries every 60 seconds. A
    /// predicate over an indexed column is both simpler and strictly more
    /// correct -- it is never up to a minute stale -- so the worker does not
    /// exist. The escrow is still `Funded` on chain either way; "expired"
    /// only means the donor's refund path has opened.
    pub fn is_expired(&self, now: i64) -> bool {
        self.state == "funded" && self.deadline <= now
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct NeedRow {
    pub escrow: String,
    pub title: String,
    pub description: String,
    pub need_hash: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ReceiptRow {
    pub receipt_hash: String,
    pub escrow: String,
    pub audio_key: String,
    pub audio_sha256: String,
    pub audio_bytes: i64,
    pub content_type: String,
    pub transcript: String,
    pub transcript_available: i64,
    pub locale: String,
    pub recorded_at: i64,
    pub created_at: i64,
}

/// Write an indexed escrow. Idempotent by pubkey, so replaying the same poll
/// after a restart is harmless.
pub async fn upsert_escrow(pool: &SqlitePool, pubkey: &Pubkey, e: &Escrow, now: i64) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO escrows
            (pubkey, donor, recipient, amount, need_hash, receipt_hash,
             deadline, state, created_at, released_at, indexed_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(pubkey) DO UPDATE SET
            receipt_hash = excluded.receipt_hash,
            state        = excluded.state,
            released_at  = excluded.released_at,
            indexed_at   = excluded.indexed_at
        "#,
    )
    .bind(pubkey.to_string())
    .bind(e.donor.to_string())
    .bind(e.recipient.to_string())
    .bind(e.amount as i64)
    .bind(hex::encode(e.need_hash))
    .bind((e.receipt_hash != cairn_core::ZERO_HASH).then(|| hex::encode(e.receipt_hash)))
    .bind(e.deadline)
    .bind(e.state.as_str())
    .bind(e.created_at)
    .bind((e.released_at != 0).then_some(e.released_at))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Default)]
pub struct EscrowFilter {
    pub donor: Option<String>,
    pub recipient: Option<String>,
    pub state: Option<EscrowState>,
    /// Only escrows past their deadline that are still Funded.
    pub expired: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list_escrows(pool: &SqlitePool, f: &EscrowFilter, now: i64) -> Result<Vec<EscrowRow>> {
    let mut q = QueryBuilder::new("SELECT * FROM escrows WHERE 1 = 1");
    if let Some(d) = &f.donor {
        q.push(" AND donor = ").push_bind(d);
    }
    if let Some(r) = &f.recipient {
        q.push(" AND recipient = ").push_bind(r);
    }
    if let Some(s) = f.state {
        q.push(" AND state = ").push_bind(s.as_str());
    }
    match f.expired {
        Some(true) => {
            q.push(" AND state = 'funded' AND deadline <= ").push_bind(now);
        }
        Some(false) => {
            q.push(" AND NOT (state = 'funded' AND deadline <= ").push_bind(now).push(")");
        }
        None => {}
    }
    q.push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(f.limit)
        .push(" OFFSET ")
        .push_bind(f.offset);

    Ok(q.build_query_as::<EscrowRow>().fetch_all(pool).await?)
}

pub async fn get_escrow(pool: &SqlitePool, pubkey: &str) -> Result<Option<EscrowRow>> {
    Ok(sqlx::query_as::<_, EscrowRow>("SELECT * FROM escrows WHERE pubkey = ?1")
        .bind(pubkey)
        .fetch_optional(pool)
        .await?)
}

pub async fn upsert_need(pool: &SqlitePool, n: &NeedRow) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO needs (escrow, title, description, need_hash, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(escrow) DO UPDATE SET
            title = excluded.title,
            description = excluded.description,
            need_hash = excluded.need_hash
        "#,
    )
    .bind(&n.escrow)
    .bind(&n.title)
    .bind(&n.description)
    .bind(&n.need_hash)
    .bind(n.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_need(pool: &SqlitePool, escrow: &str) -> Result<Option<NeedRow>> {
    Ok(sqlx::query_as::<_, NeedRow>("SELECT * FROM needs WHERE escrow = ?1")
        .bind(escrow)
        .fetch_optional(pool)
        .await?)
}

pub async fn needs_for(pool: &SqlitePool, escrows: &[String]) -> Result<Vec<NeedRow>> {
    if escrows.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = QueryBuilder::new("SELECT * FROM needs WHERE escrow IN (");
    let mut sep = q.separated(", ");
    for e in escrows {
        sep.push_bind(e);
    }
    q.push(")");
    Ok(q.build_query_as::<NeedRow>().fetch_all(pool).await?)
}

pub async fn insert_receipt(pool: &SqlitePool, r: &ReceiptRow) -> Result<()> {
    // Content-addressed on both sides: an identical recording for an identical
    // escrow produces an identical receipt_hash, so a client retrying an
    // upload after a dropped connection reuses the row rather than duplicating.
    sqlx::query(
        r#"
        INSERT INTO receipts
            (receipt_hash, escrow, audio_key, audio_sha256, audio_bytes, content_type,
             transcript, transcript_available, locale, recorded_at, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(receipt_hash) DO NOTHING
        "#,
    )
    .bind(&r.receipt_hash)
    .bind(&r.escrow)
    .bind(&r.audio_key)
    .bind(&r.audio_sha256)
    .bind(r.audio_bytes)
    .bind(&r.content_type)
    .bind(&r.transcript)
    .bind(r.transcript_available)
    .bind(&r.locale)
    .bind(r.recorded_at)
    .bind(r.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_receipt(pool: &SqlitePool, receipt_hash: &str) -> Result<Option<ReceiptRow>> {
    Ok(sqlx::query_as::<_, ReceiptRow>("SELECT * FROM receipts WHERE receipt_hash = ?1")
        .bind(receipt_hash)
        .fetch_optional(pool)
        .await?)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Stats {
    pub total_locked_lamports: i64,
    pub total_released_lamports: i64,
    pub total_refunded_lamports: i64,
    pub awaiting_receipt: i64,
    pub expired_pending_refund: i64,
    pub released_count: i64,
    pub refunded_count: i64,
    pub escrow_count: i64,
    pub last_indexed_at: Option<i64>,
}

pub async fn stats(pool: &SqlitePool, now: i64) -> Result<Stats> {
    let row = sqlx::query(
        r#"
        SELECT
          COALESCE(SUM(CASE WHEN state = 'funded'   THEN amount END), 0) AS locked,
          COALESCE(SUM(CASE WHEN state = 'released' THEN amount END), 0) AS released,
          COALESCE(SUM(CASE WHEN state = 'refunded' THEN amount END), 0) AS refunded,
          COUNT(CASE WHEN state = 'funded' AND deadline >  ?1 THEN 1 END) AS awaiting,
          COUNT(CASE WHEN state = 'funded' AND deadline <= ?1 THEN 1 END) AS expired,
          COUNT(CASE WHEN state = 'released' THEN 1 END) AS released_n,
          COUNT(CASE WHEN state = 'refunded' THEN 1 END) AS refunded_n,
          COUNT(*) AS total,
          MAX(indexed_at) AS last_indexed
        FROM escrows
        "#,
    )
    .bind(now)
    .fetch_one(pool)
    .await?;

    Ok(Stats {
        total_locked_lamports: row.try_get("locked")?,
        total_released_lamports: row.try_get("released")?,
        total_refunded_lamports: row.try_get("refunded")?,
        awaiting_receipt: row.try_get("awaiting")?,
        expired_pending_refund: row.try_get("expired")?,
        released_count: row.try_get("released_n")?,
        refunded_count: row.try_get("refunded_n")?,
        escrow_count: row.try_get("total")?,
        last_indexed_at: row.try_get("last_indexed")?,
    })
}
