//! The HTTP wire format, as types.
//!
//! These live in the library rather than inside the route handlers because
//! `cairn-cli` deserialises them. One definition of each response shape,
//! shared by the code that writes it and the code that reads it -- the same
//! reason `cairn-core` holds the hash and the account layout.
//!
//! When the frontend was TypeScript this could not be done, and the client
//! carried its own hand-written mirror of every response. Deleting the
//! browser deleted the mirror.

use serde::{Deserialize, Serialize};

use crate::db::{EscrowRow, NeedRow, ReceiptRow};

/// An escrow plus the two things the chain does not store: whether its
/// deadline has passed, and the human-readable need text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscrowView {
    #[serde(flatten)]
    pub escrow: EscrowRow,
    pub expired: bool,
    /// `None` when nobody registered a description. The escrow is still
    /// perfectly valid -- the `need_hash` on chain is the commitment, and the
    /// text is a convenience.
    pub need: Option<NeedRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscrowList {
    pub escrows: Vec<EscrowView>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscrowDetail {
    pub escrow: EscrowView,
    pub receipt: Option<ReceiptRow>,
    pub audio_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNeed {
    pub escrow: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Challenge {
    pub nonce: String,
    pub expires_in_seconds: u64,
    /// The exact bytes to sign. Handed back verbatim so the client signs what
    /// the server will verify, instead of rebuilding the format and getting a
    /// newline wrong.
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedReceipt {
    pub receipt_hash: String,
    pub audio_sha256: String,
    pub audio_url: String,
    pub transcript: String,
    pub transcript_available: bool,
    pub locale: String,
    pub recorded_at: i64,
}

/// What `/v1/verify/{signature}` answers.
///
/// Most fields are optional because a verdict of `artifacts_unknown` or
/// `transaction_failed` has nothing to report for them. That is deliberate:
/// the alternative is filling them with zeroes, and a zeroed hash that reads
/// like a real answer is exactly the failure this endpoint exists to prevent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub signature: String,
    pub verdict: Verdict,
    /// Whether recomputation reproduced the hash committed on chain.
    #[serde(rename = "match")]
    pub matches: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub escrow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub onchain_receipt_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub recomputed_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub recorded_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub slot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub block_time: Option<i64>,
    pub audio_available: bool,
    pub transcript_available: bool,
    pub verified_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Audio re-downloaded, re-hashed, and the receipt reproduces.
    Verified,
    /// Recomputation does not reproduce the committed hash.
    Mismatch,
    /// Transcript and metadata reproduce, but the recording could not be
    /// fetched -- so this run could not re-derive the hash from the bytes.
    AudioUnavailable,
    /// A hash is committed on chain; this server holds nothing for it.
    ArtifactsUnknown,
    /// The transaction itself failed, so it committed nothing.
    TransactionFailed,
}

impl Verdict {
    pub fn detail(self) -> &'static str {
        match self {
            Verdict::Verified => {
                "the recording was re-downloaded, re-hashed, and reproduces the hash \
                 committed on chain by the recipient"
            }
            Verdict::AudioUnavailable => {
                "the transcript and metadata reproduce the committed hash, but the recording \
                 itself could not be fetched, so this run could not re-derive it from the audio"
            }
            Verdict::Mismatch => {
                "recomputation does not reproduce the hash committed on chain; the stored \
                 artifacts are not the ones the recipient signed for"
            }
            Verdict::ArtifactsUnknown => {
                "a receipt hash is committed on chain, but this server holds no recording or \
                 transcript for it, so there is nothing to recompute"
            }
            Verdict::TransactionFailed => {
                "the transaction itself failed on chain, so it committed nothing"
            }
        }
    }
}
