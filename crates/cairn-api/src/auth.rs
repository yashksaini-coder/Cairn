//! Upload authorization.
//!
//! Read §2 before changing anything here. This challenge proves the caller
//! controls the recipient key *before the server spends money on
//! transcription*. It is spam and cost control, nothing more.
//!
//! It is not what authorizes the release of funds. That happens on-chain, in
//! `submit_receipt`, against a signature this server never sees and could not
//! forge if it did. If this entire module were bypassed, the worst outcome is
//! a wasted Scribe call.

use crate::error::{ApiError, ApiResult};
use ed25519_dalek::{Signature, VerifyingKey};
use rand::RngCore;
use solana_program::pubkey::Pubkey;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Domain-separated so a Cairn challenge signature can never be replayed as a
/// signature over anything else the wallet is asked to sign.
pub const CHALLENGE_PREFIX: &str = "cairn:challenge:v1";

/// The exact bytes the wallet signs. Mirrored in `web/lib/cairn.ts`.
pub fn challenge_message(escrow: &Pubkey, nonce: &str) -> String {
    format!("{CHALLENGE_PREFIX}\n{escrow}\n{nonce}")
}

struct Pending {
    escrow: Pubkey,
    expires_at: Instant,
}

/// In-memory, single-process, and that is the right call: a challenge lives
/// for 120 seconds and losing every outstanding one on deploy costs a user
/// one retry.
///
/// ponytail: single-process nonce store; move to Redis only if the API is
/// ever run as more than one replica, at which point a challenge issued by
/// one instance would fail against another.
pub struct Challenges {
    inner: Mutex<HashMap<String, Pending>>,
    ttl: Duration,
}

impl Challenges {
    pub fn new(ttl: Duration) -> Self {
        Self { inner: Mutex::new(HashMap::new()), ttl }
    }

    pub fn issue(&self, escrow: Pubkey) -> (String, u64) {
        let mut bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut bytes);
        let nonce = bs58::encode(bytes).into_string();

        let mut guard = self.inner.lock().expect("challenge store poisoned");
        // Swept on write rather than on a timer: the map only grows when
        // someone is actively uploading, so there is nothing to reap in the
        // idle case and no task to supervise.
        let now = Instant::now();
        guard.retain(|_, p| p.expires_at > now);
        guard.insert(nonce.clone(), Pending { escrow, expires_at: now + self.ttl });

        (nonce, self.ttl.as_secs())
    }

    /// Single-use: the nonce is removed whether or not verification succeeds,
    /// so a captured challenge cannot be brute-forced against.
    pub fn verify(
        &self,
        nonce: &str,
        escrow: &Pubkey,
        signer: &Pubkey,
        signature_b58: &str,
    ) -> ApiResult<()> {
        let pending = {
            let mut guard = self.inner.lock().expect("challenge store poisoned");
            guard.remove(nonce)
        };

        let pending = pending
            .ok_or_else(|| ApiError::Unauthorized("challenge is unknown or already used".into()))?;

        if pending.expires_at < Instant::now() {
            return Err(ApiError::Unauthorized("challenge expired; request a new one".into()));
        }
        if pending.escrow != *escrow {
            return Err(ApiError::Unauthorized(
                "challenge was issued for a different escrow".into(),
            ));
        }

        verify_signature(&challenge_message(escrow, nonce), signer, signature_b58)
    }
}

pub fn verify_signature(message: &str, signer: &Pubkey, signature_b58: &str) -> ApiResult<()> {
    let sig_bytes: [u8; 64] =
        bs58::decode(signature_b58).into_vec().ok().and_then(|v| v.try_into().ok()).ok_or_else(
            || ApiError::BadRequest("signature is not 64 base58-encoded bytes".into()),
        )?;

    let key = VerifyingKey::from_bytes(&signer.to_bytes())
        .map_err(|_| ApiError::BadRequest("signer is not a valid ed25519 public key".into()))?;

    // `verify_strict` rejects small-order and non-canonical points. The
    // permissive variant would accept signatures that a different verifier
    // rejects, and receipts must mean the same thing to everyone.
    key.verify_strict(message.as_bytes(), &Signature::from_bytes(&sig_bytes))
        .map_err(|_| ApiError::Unauthorized("signature does not match the recipient key".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn keypair() -> (SigningKey, Pubkey) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = Pubkey::new_from_array(sk.verifying_key().to_bytes());
        (sk, pk)
    }

    #[test]
    fn round_trip() {
        let (sk, signer) = keypair();
        let escrow = Pubkey::new_from_array([3u8; 32]);
        let store = Challenges::new(Duration::from_secs(120));
        let (nonce, _) = store.issue(escrow);

        let sig = bs58::encode(sk.sign(challenge_message(&escrow, &nonce).as_bytes()).to_bytes())
            .into_string();
        assert!(store.verify(&nonce, &escrow, &signer, &sig).is_ok());
        // ...and the same nonce cannot be spent twice.
        assert!(store.verify(&nonce, &escrow, &signer, &sig).is_err());
    }

    #[test]
    fn a_signature_for_one_escrow_does_not_work_on_another() {
        let (sk, signer) = keypair();
        let (a, b) = (Pubkey::new_from_array([1u8; 32]), Pubkey::new_from_array([2u8; 32]));
        let store = Challenges::new(Duration::from_secs(120));
        let (nonce, _) = store.issue(a);
        let sig = bs58::encode(sk.sign(challenge_message(&b, &nonce).as_bytes()).to_bytes())
            .into_string();
        assert!(store.verify(&nonce, &a, &signer, &sig).is_err());
    }

    #[test]
    fn rejects_a_stranger() {
        let (sk, _) = keypair();
        let escrow = Pubkey::new_from_array([3u8; 32]);
        let store = Challenges::new(Duration::from_secs(120));
        let (nonce, _) = store.issue(escrow);
        let sig = bs58::encode(sk.sign(challenge_message(&escrow, &nonce).as_bytes()).to_bytes())
            .into_string();
        let stranger = Pubkey::new_from_array([9u8; 32]);
        assert!(store.verify(&nonce, &escrow, &stranger, &sig).is_err());
    }

    #[test]
    fn expired_challenges_are_refused() {
        let (sk, signer) = keypair();
        let escrow = Pubkey::new_from_array([3u8; 32]);
        let store = Challenges::new(Duration::from_millis(0));
        let (nonce, _) = store.issue(escrow);
        let sig = bs58::encode(sk.sign(challenge_message(&escrow, &nonce).as_bytes()).to_bytes())
            .into_string();
        assert!(store.verify(&nonce, &escrow, &signer, &sig).is_err());
    }
}
