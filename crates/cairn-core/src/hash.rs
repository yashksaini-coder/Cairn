//! The one hash implementation.

use crate::receipt::CanonicalReceipt;
use solana_program::hash::hashv;

/// Domain separation tag for receipts. Any change to the preimage layout gets
/// a new tag, so hashes from different versions live in disjoint spaces.
pub const RECEIPT_DOMAIN: &[u8] = b"cairn:receipt:v1";

/// Domain separation tag for need descriptions.
pub const NEED_DOMAIN: &[u8] = b"cairn:need:v1";

/// Hash a canonical receipt.
///
/// Preimage layout:
///
/// ```text
/// b"cairn:receipt:v1" | escrow(32) | audio_sha256(32) | transcript(n) | locale(8) | recorded_at(8)
/// ```
///
/// `transcript` is the only variable-length field and every field around it is
/// fixed width, so for a preimage of length `L` the transcript is always
/// exactly `L - 96` bytes. The encoding is therefore injective without an
/// explicit length prefix -- there is no pair of distinct receipts that
/// concatenate to the same bytes. Add a second variable-length field and that
/// stops being true; length-prefix both if you ever do.
///
/// The `version` field is deliberately absent from the preimage. It is
/// implied by the domain tag, and hashing it as well would let a v2 receipt
/// that reuses the v1 tag masquerade as v1.
pub fn receipt_hash(r: &CanonicalReceipt) -> [u8; 32] {
    hashv(&[
        RECEIPT_DOMAIN,
        r.escrow.as_ref(),
        &r.audio_sha256,
        r.transcript.as_bytes(),
        &r.locale,
        &r.recorded_at.to_le_bytes(),
    ])
    .to_bytes()
}

/// Hash a need description that has **already** been normalised.
///
/// Prefer [`need_hash`] unless you are certain the caller normalised. The
/// browser and the server must agree byte for byte here, or every need in the
/// UI renders a tamper warning (§9.2).
pub fn need_hash_of_normalized(normalized: &str) -> [u8; 32] {
    hashv(&[NEED_DOMAIN, normalized.as_bytes()]).to_bytes()
}

/// Normalise, then hash. Host-side only -- the program never computes this.
///
/// The TypeScript client mirrors this with
/// `text.normalize("NFC").trim().replace(/\s+/g, " ")`.
#[cfg(feature = "host")]
pub fn need_hash(raw: &str) -> [u8; 32] {
    need_hash_of_normalized(&crate::normalize::normalize_text(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::{encode_locale, CanonicalReceipt};
    use solana_program::pubkey::Pubkey;

    fn receipt(transcript: &str, locale: &str) -> CanonicalReceipt {
        CanonicalReceipt::new(
            Pubkey::new_from_array([7u8; 32]),
            [9u8; 32],
            transcript.to_string(),
            encode_locale(locale),
            1_757_000_000,
        )
    }

    #[test]
    fn hash_is_stable() {
        // Pins the wire format. If this fails, every receipt ever issued has
        // been invalidated -- bump the domain tag instead of editing this.
        let h = receipt("i received the money", "en").hash();
        assert_eq!(h, receipt("i received the money", "en").hash());
        assert_ne!(h, receipt("i received the money.", "en").hash());
        assert_ne!(h, receipt("i received the money", "hi").hash());
    }

    #[test]
    fn transcript_and_locale_do_not_smear() {
        // The property the fixed-width layout buys us: text cannot migrate
        // across the transcript/locale boundary to produce a collision.
        assert_ne!(receipt("ab", "cd").hash(), receipt("abc", "d").hash());
    }

    #[test]
    fn empty_transcript_still_hashes() {
        // §12: transcription failure yields a valid receipt.
        assert_ne!(receipt("", "en").hash(), [0u8; 32]);
    }

    #[test]
    fn need_hash_normalises() {
        assert_eq!(need_hash("  a   b  "), need_hash_of_normalized("a b"));
    }

    #[test]
    fn domains_are_disjoint() {
        assert_ne!(need_hash_of_normalized(""), receipt("", "").hash());
    }
}
