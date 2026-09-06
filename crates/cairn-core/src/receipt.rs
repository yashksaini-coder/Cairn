//! The canonical receipt: what the recipient's voice recording *is*, reduced
//! to the exact bytes that get hashed.

use solana_program::pubkey::Pubkey;

/// Bumped only if the hash preimage changes shape. The version is carried by
/// the domain tag in [`crate::hash::RECEIPT_DOMAIN`] rather than hashed as a
/// field, so a v2 receipt can never collide with a v1 one.
pub const RECEIPT_VERSION: u8 = 1;

/// BCP-47 tag, null-padded. Fixed width on purpose -- see the note on
/// preimage injectivity in [`crate::hash::receipt_hash`].
pub const LOCALE_LEN: usize = 8;

/// Everything that is bound to a release transaction.
///
/// The recording itself is not here; `audio_sha256` stands in for it. That is
/// the whole trick: the blob store is replaceable infrastructure, and losing
/// the audio degrades a verification to "unavailable" rather than "forged".
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "host", derive(serde::Serialize, serde::Deserialize))]
pub struct CanonicalReceipt {
    pub version: u8,
    pub escrow: Pubkey,
    /// SHA-256 of the raw audio bytes, exactly as uploaded.
    pub audio_sha256: [u8; 32],
    /// UTF-8, NFC-normalised, trimmed, internal whitespace collapsed.
    /// Empty when transcription failed -- an empty transcript is a valid
    /// receipt (§12), not an error.
    pub transcript: String,
    pub locale: [u8; LOCALE_LEN],
    pub recorded_at: i64,
}

impl CanonicalReceipt {
    pub fn new(
        escrow: Pubkey,
        audio_sha256: [u8; 32],
        transcript: String,
        locale: [u8; LOCALE_LEN],
        recorded_at: i64,
    ) -> Self {
        Self { version: RECEIPT_VERSION, escrow, audio_sha256, transcript, locale, recorded_at }
    }

    /// Compute this receipt's hash. Thin alias for [`crate::hash::receipt_hash`].
    pub fn hash(&self) -> [u8; 32] {
        crate::hash::receipt_hash(self)
    }
}

/// Pack a BCP-47 tag such as `"en-IN"` into the fixed-width locale field.
///
/// Tags longer than 8 bytes are truncated rather than rejected: the locale is
/// descriptive metadata, and refusing to issue a receipt over a long language
/// tag would be a worse failure than recording a shortened one.
pub fn encode_locale(tag: &str) -> [u8; LOCALE_LEN] {
    let mut out = [0u8; LOCALE_LEN];
    let bytes = tag.as_bytes();
    let n = core::cmp::min(bytes.len(), LOCALE_LEN);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// Inverse of [`encode_locale`], for display. Invalid UTF-8 yields `""`.
pub fn decode_locale(locale: &[u8; LOCALE_LEN]) -> &str {
    let end = locale.iter().position(|b| *b == 0).unwrap_or(LOCALE_LEN);
    core::str::from_utf8(&locale[..end]).unwrap_or("")
}
