use axum::{
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cairn_core::{receipt::encode_locale, CanonicalReceipt};
use serde::Deserialize;
use serde_json::json;
use solana_program::{hash::hash as sha256, pubkey::Pubkey};
use std::str::FromStr;

use crate::{
    blob::BlobStore,
    db::{self, ReceiptRow},
    error::{ApiError, ApiResult},
    state::{now, AppState},
    views::{Challenge, IssuedReceipt},
};

#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    pub escrow: String,
}

/// Step 1 of §8.2. Hand out a nonce bound to one escrow.
pub async fn challenge(
    State(s): State<AppState>,
    Json(body): Json<ChallengeRequest>,
) -> ApiResult<Json<Challenge>> {
    let escrow = parse_pubkey(&body.escrow, "escrow")?;
    let (nonce, expires_in_seconds) = s.challenges.issue(escrow);
    let message = crate::auth::challenge_message(&escrow, &nonce);

    Ok(Json(Challenge { nonce, expires_in_seconds, message }))
}

#[derive(Default)]
struct Upload {
    escrow: Option<String>,
    nonce: Option<String>,
    signature: Option<String>,
    recorded_at: Option<i64>,
    duration_ms: Option<i64>,
    locale_hint: Option<String>,
    filename: Option<String>,
    content_type: Option<String>,
    audio: Option<Vec<u8>>,
}

/// The receipt pipeline (§8.3).
///
/// The ordering below is load-bearing. The blob is written *before* the hash
/// is returned to be signed, so a transaction that later fails leaves an
/// orphaned object -- harmless, garbage-collected. Doing it the other way
/// round would let a committed on-chain hash point at bytes that were never
/// stored, which is not recoverable and would make a receipt permanently
/// unverifiable.
pub async fn upload(
    State(s): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<Json<IssuedReceipt>> {
    let mut up = Upload::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart body: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" | "audio" => {
                up.filename = field.file_name().map(str::to_string);
                up.content_type = field.content_type().map(str::to_string);
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("could not read audio: {e}")))?;
                up.audio = Some(bytes.to_vec());
            }
            _ => {
                let value = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("field {name} is not text: {e}")))?;
                match name.as_str() {
                    "escrow" => up.escrow = Some(value),
                    "nonce" => up.nonce = Some(value),
                    "signature" => up.signature = Some(value),
                    "recorded_at" => up.recorded_at = value.parse().ok(),
                    "duration_ms" => up.duration_ms = value.parse().ok(),
                    "locale" => up.locale_hint = Some(value),
                    other => tracing::debug!(field = other, "ignoring unexpected form field"),
                }
            }
        }
    }

    let escrow_str = required(up.escrow, "escrow")?;
    let escrow_key = parse_pubkey(&escrow_str, "escrow")?;
    let audio = required(up.audio, "file")?;
    let content_type = up.content_type.unwrap_or_else(|| "audio/webm".to_string());

    // The escrow must already be indexed, which also means it exists on
    // chain. `recipient` is immutable after creation, so reading it from the
    // read model rather than from RPC cannot be made stale in a way that
    // matters -- there is no update that could change who is allowed here.
    let row = db::get_escrow(&s.pool, &escrow_str).await?.ok_or_else(|| {
        ApiError::NotFound("that escrow is not indexed yet; wait for confirmation and retry".into())
    })?;
    let recipient = parse_pubkey(&row.recipient, "recipient")?;

    s.challenges.verify(
        &required(up.nonce, "nonce")?,
        &escrow_key,
        &recipient,
        &required(up.signature, "signature")?,
    )?;

    if row.state != "funded" {
        return Err(ApiError::BadRequest(format!(
            "escrow is {}; a receipt can only be attached to a funded escrow",
            row.state
        )));
    }

    validate_audio(&s, &audio, up.duration_ms)?;

    // 1. Content address. `hashv` over a single slice is plain SHA-256, and
    //    it is the same function the program would use, from the same crate.
    let audio_sha256 = sha256(&audio).to_bytes();
    let audio_bytes = audio.len() as i64;
    let key = BlobStore::key_for(&audio_sha256, &content_type);

    // 2. Store, before anything is signable.
    s.blobs.put(&key, audio.clone(), &content_type).await.map_err(ApiError::Internal)?;

    // 3. Transcribe. Failure here is not failure of the upload (§12): the
    //    recipient still gets paid, the receipt is still bound to the audio,
    //    and the transcript is simply marked unavailable.
    let filename = up.filename.unwrap_or_else(|| "receipt.webm".to_string());
    let transcript = match s.transcriber.transcribe(audio, &filename, &content_type).await {
        Ok(Some(t)) => Some(t),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = ?e, escrow = %escrow_str, "transcription failed; issuing an empty transcript");
            None
        }
    };

    let transcript_available = transcript.is_some();
    let (text, locale) = match transcript {
        Some(t) => (cairn_core::normalize::normalize_text(&t.text), t.locale),
        None => (String::new(), up.locale_hint.unwrap_or_else(|| "und".into())),
    };

    // 4. Canonical form and the one hash function.
    let recorded_at = up.recorded_at.unwrap_or_else(now);
    let receipt = CanonicalReceipt::new(
        escrow_key,
        audio_sha256,
        text.clone(),
        encode_locale(&locale),
        recorded_at,
    );
    let receipt_hash = receipt.hash();
    let receipt_hex = hex::encode(receipt_hash);

    db::insert_receipt(
        &s.pool,
        &ReceiptRow {
            receipt_hash: receipt_hex.clone(),
            escrow: escrow_str,
            audio_key: key.clone(),
            audio_sha256: hex::encode(audio_sha256),
            audio_bytes,
            content_type,
            transcript: text.clone(),
            transcript_available: transcript_available as i64,
            locale: locale.clone(),
            recorded_at,
            created_at: now(),
        },
    )
    .await?;

    // The client prints this next to the transcript, recomputes it locally
    // from the audio file it still holds, and only then asks for a signature.
    // The recipient is attesting to these exact words.
    Ok(Json(IssuedReceipt {
        receipt_hash: receipt_hex,
        audio_sha256: hex::encode(audio_sha256),
        audio_url: s.blobs.public_url(&key),
        transcript: text,
        transcript_available,
        locale,
        recorded_at,
    }))
}

pub async fn get_one(
    State(s): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let receipt = db::get_receipt(&s.pool, &hash.to_lowercase())
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no receipt artifact stored for {hash}")))?;
    let audio_url = s.blobs.public_url(&receipt.audio_key);
    Ok(Json(json!({ "receipt": receipt, "audio_url": audio_url })))
}

/// Serves audio from the local store. With R2 configured, `public_url` points
/// at the bucket and this route is never hit.
pub async fn blob(State(s): State<AppState>, Path(key): Path<String>) -> Response {
    match s.blobs.get(&key).await {
        Ok(Some(bytes)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime_for(&key)),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => ApiError::NotFound("no such recording".into()).into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

fn validate_audio(s: &AppState, audio: &[u8], duration_ms: Option<i64>) -> ApiResult<()> {
    let cfg = &s.cfg;
    if audio.len() > cfg.max_audio_bytes {
        return Err(ApiError::PayloadTooLarge { got: audio.len(), cap: cfg.max_audio_bytes });
    }
    // Duration is checked twice, from two untrusting angles. The client's
    // reported `duration_ms` is convenient but forgeable; the byte floor is
    // crude but cannot be talked around. Neither alone is enough, and reading
    // the true duration would mean demuxing webm/opus server-side for a
    // 3-second minimum that exists to stop empty uploads.
    //
    // ponytail: byte-count proxy for duration. Add a demuxer only if someone
    // starts gaming the floor with padded silence.
    if audio.len() < cfg.min_audio_bytes {
        return Err(ApiError::BadRequest(format!(
            "recording is too short; speak for at least {} seconds",
            cfg.min_audio_ms / 1000
        )));
    }
    if let Some(ms) = duration_ms {
        if ms < cfg.min_audio_ms {
            return Err(ApiError::BadRequest(format!(
                "recording is {ms}ms; the minimum is {}ms",
                cfg.min_audio_ms
            )));
        }
        if ms > cfg.max_audio_ms {
            return Err(ApiError::BadRequest(format!(
                "recording is {ms}ms; the maximum is {}ms",
                cfg.max_audio_ms
            )));
        }
    }
    Ok(())
}

fn required<T>(value: Option<T>, name: &str) -> ApiResult<T> {
    value.ok_or_else(|| ApiError::BadRequest(format!("{name} is required")))
}

fn parse_pubkey(raw: &str, field: &str) -> ApiResult<Pubkey> {
    Pubkey::from_str(raw.trim())
        .map_err(|_| ApiError::BadRequest(format!("{field} is not a valid pubkey: {raw}")))
}

fn mime_for(key: &str) -> &'static str {
    match key.rsplit('.').next().unwrap_or("") {
        "webm" => "audio/webm",
        "ogg" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}
