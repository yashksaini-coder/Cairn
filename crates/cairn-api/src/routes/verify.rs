//! Independent verification (§8.4).
//!
//! This endpoint is the project's central claim and the one thing §15 says is
//! never cut. It answers a narrow question: *does the hash committed in this
//! release transaction actually equal the hash of the artifacts on file?*
//!
//! It answers it by recomputing from source. It re-reads the transaction from
//! an RPC node, re-downloads the audio, re-hashes the bytes it got, rebuilds
//! the canonical receipt and runs it through the same `cairn-core` function
//! the program committed to. Nothing in the path trusts a stored hash, and
//! nothing in the path trusts this server's own earlier arithmetic.
//!
//! A third party can do all of this without us. That is the point; the
//! endpoint is a convenience, not an authority.

use axum::{
    extract::{Path, State},
    Json,
};
use cairn_core::{receipt::encode_locale, CanonicalReceipt};
use serde_json::Value;
use solana_program::{hash::hash as sha256, pubkey::Pubkey};
use std::str::FromStr;

use crate::{
    db,
    error::{ApiError, ApiResult},
    state::AppState,
    views::{Verdict, Verification},
};

/// `sha256("global:submit_receipt")[..8]`, the Anchor instruction selector.
const SUBMIT_RECEIPT_IX: [u8; 8] = [172, 84, 119, 35, 195, 154, 214, 176];

fn bare(signature: &str, verdict: Verdict) -> Verification {
    Verification {
        signature: signature.to_string(),
        verdict,
        matches: false,
        detail: verdict.detail().to_string(),
        escrow: None,
        onchain_receipt_hash: None,
        recomputed_hash: None,
        audio_sha256: None,
        audio_url: None,
        transcript: None,
        locale: None,
        recorded_at: None,
        slot: None,
        block_time: None,
        audio_available: false,
        transcript_available: false,
        verified_at: chrono::Utc::now().to_rfc3339(),
    }
}

pub async fn verify(
    State(s): State<AppState>,
    Path(signature): Path<String>,
) -> ApiResult<Json<Verification>> {
    let tx = s
        .rpc
        .transaction(&signature)
        .await
        .map_err(|e| ApiError::Upstream { service: "solana-rpc", detail: e.to_string() })?
        .ok_or_else(|| {
            ApiError::NotFound(
                "this RPC node has no such transaction; it may be older than the node's history"
                    .into(),
            )
        })?;

    if !tx["meta"]["err"].is_null() {
        return Ok(Json(bare(&signature, Verdict::TransactionFailed)));
    }

    let (escrow, onchain_hash) = extract_commitment(&tx, &s.cfg.program_id)?;
    let onchain_hex = hex::encode(onchain_hash);

    // The artifacts. Absence here is a gap in *our* records, not evidence
    // against the receipt -- the commitment on chain stands either way.
    let Some(record) = db::get_receipt(&s.pool, &onchain_hex).await? else {
        return Ok(Json(Verification {
            escrow: Some(escrow.to_string()),
            onchain_receipt_hash: Some(onchain_hex),
            ..bare(&signature, Verdict::ArtifactsUnknown)
        }));
    };

    // Re-derive the audio hash from the bytes we can actually fetch. Falling
    // back to the stored value when the blob is gone is deliberate and
    // deliberately labelled: it still proves the transcript and metadata are
    // bound to the commitment, but it no longer proves the recording is.
    let fetched = s.blobs.get(&record.audio_key).await.map_err(ApiError::Internal)?;
    let audio_available = fetched.is_some();
    let audio_sha256: [u8; 32] = match &fetched {
        Some(bytes) => sha256(bytes).to_bytes(),
        None => decode_hash(&record.audio_sha256)?,
    };

    let recomputed = CanonicalReceipt::new(
        escrow,
        audio_sha256,
        record.transcript.clone(),
        encode_locale(&record.locale),
        record.recorded_at,
    )
    .hash();
    let matches = recomputed == onchain_hash;

    // A corrupted blob lands here: the bytes hash to something else, the
    // receipt hash no longer reproduces, and the verdict is `mismatch` --
    // distinct from `audio_unavailable`, which is what a *missing* blob gives
    // (§8.4, demo beat 6).
    let verdict = match (matches, audio_available) {
        (true, true) => Verdict::Verified,
        (true, false) => Verdict::AudioUnavailable,
        (false, _) => Verdict::Mismatch,
    };

    Ok(Json(Verification {
        signature,
        verdict,
        matches,
        detail: verdict.detail().to_string(),
        escrow: Some(escrow.to_string()),
        onchain_receipt_hash: Some(onchain_hex),
        recomputed_hash: Some(hex::encode(recomputed)),
        audio_sha256: Some(hex::encode(audio_sha256)),
        audio_url: Some(s.blobs.public_url(&record.audio_key)),
        transcript: Some(record.transcript),
        locale: Some(record.locale),
        recorded_at: Some(record.recorded_at),
        slot: tx["slot"].as_u64(),
        block_time: tx["blockTime"].as_i64(),
        audio_available,
        transcript_available: record.transcript_available != 0,
        verified_at: chrono::Utc::now().to_rfc3339(),
    }))
}

/// Pull the escrow and the committed hash out of a confirmed transaction.
///
/// The account layout of `SubmitReceipt` is part of the wire format here:
/// escrow is the first account. Reordering it in the program would silently
/// invalidate verification of every receipt issued before the change.
fn extract_commitment(tx: &Value, program_id: &Pubkey) -> ApiResult<(Pubkey, [u8; 32])> {
    let message = &tx["transaction"]["message"];

    // Static keys, then anything an address lookup table contributed. Cairn's
    // own transactions never use lookup tables, but a wallet is free to
    // bundle the instruction into one that does.
    let mut keys: Vec<String> = message["accountKeys"]
        .as_array()
        .ok_or_else(|| malformed("transaction has no accountKeys"))?
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    for section in ["writable", "readonly"] {
        if let Some(extra) = tx["meta"]["loadedAddresses"][section].as_array() {
            keys.extend(extra.iter().filter_map(|k| k.as_str().map(str::to_string)));
        }
    }

    let instructions = message["instructions"]
        .as_array()
        .ok_or_else(|| malformed("transaction has no instructions"))?;

    for ix in instructions {
        let program_index = ix["programIdIndex"].as_u64().unwrap_or(u64::MAX) as usize;
        if keys.get(program_index).map(String::as_str) != Some(&program_id.to_string()) {
            continue;
        }

        let data = ix["data"].as_str().unwrap_or_default();
        let decoded = bs58::decode(data).into_vec().unwrap_or_default();
        if decoded.len() < 40 || decoded[..8] != SUBMIT_RECEIPT_IX {
            continue;
        }

        let escrow_index = ix["accounts"][0].as_u64().unwrap_or(u64::MAX) as usize;
        let escrow = keys
            .get(escrow_index)
            .ok_or_else(|| malformed("submit_receipt names an account outside the key table"))?;

        let mut hash = [0u8; 32];
        hash.copy_from_slice(&decoded[8..40]);
        return Ok((
            Pubkey::from_str(escrow).map_err(|_| malformed("escrow key is not a pubkey"))?,
            hash,
        ));
    }

    Err(ApiError::BadRequest(
        "this transaction contains no Cairn submit_receipt instruction; \
         verification only applies to release transactions"
            .into(),
    ))
}

fn malformed(what: &str) -> ApiError {
    ApiError::Upstream { service: "solana-rpc", detail: what.to_string() }
}

fn decode_hash(hex_str: &str) -> ApiResult<[u8; 32]> {
    hex::decode(hex_str)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("stored hash {hex_str} is malformed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tx_with(program: &str, data_bytes: Vec<u8>) -> Value {
        json!({
            "slot": 1,
            "meta": { "err": null },
            "transaction": { "message": {
                "accountKeys": ["EscRow1111111111111111111111111111111111111", program],
                "instructions": [{ "programIdIndex": 1, "accounts": [0, 0, 0, 0],
                                   "data": bs58::encode(data_bytes).into_string() }],
            }},
        })
    }

    #[test]
    fn finds_the_commitment() {
        let program = Pubkey::new_unique();
        let mut data = SUBMIT_RECEIPT_IX.to_vec();
        data.extend([9u8; 32]);
        let tx = tx_with(&program.to_string(), data);
        let (escrow, hash) = extract_commitment(&tx, &program).unwrap();
        assert_eq!(hash, [9u8; 32]);
        assert_eq!(escrow.to_string(), "EscRow1111111111111111111111111111111111111");
    }

    #[test]
    fn ignores_a_different_instruction_on_the_same_program() {
        let program = Pubkey::new_unique();
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend([9u8; 32]);
        assert!(extract_commitment(&tx_with(&program.to_string(), data), &program).is_err());
    }

    #[test]
    fn ignores_another_program_entirely() {
        let mut data = SUBMIT_RECEIPT_IX.to_vec();
        data.extend([9u8; 32]);
        let tx = tx_with(&Pubkey::new_unique().to_string(), data);
        assert!(extract_commitment(&tx, &Pubkey::new_unique()).is_err());
    }

    #[test]
    fn rejects_a_truncated_instruction() {
        let program = Pubkey::new_unique();
        let tx = tx_with(&program.to_string(), SUBMIT_RECEIPT_IX.to_vec());
        assert!(extract_commitment(&tx, &program).is_err());
    }
}
