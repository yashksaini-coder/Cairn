//! Cairn's shared core.
//!
//! This crate is the reason the project's central claim holds. It compiles to
//! both `sbf-solana-solana` and the host target, so the hash the on-chain
//! program commits to and the hash the server computes come out of the same
//! function. There is no second implementation to drift.
//!
//! Constraints, enforced by CI (`E4`):
//!
//! * no I/O, no async, no networking;
//! * hashing goes through [`solana_program::hash::hashv`], which is the
//!   `sol_sha256` syscall on-chain and the `sha2` crate off-chain;
//! * anything host-only (Unicode normalisation, serde, hex) lives behind the
//!   `host` feature so it never reaches the SBF binary.

pub mod error;
pub mod hash;
pub mod receipt;
pub mod seeds;
pub mod state;

#[cfg(feature = "host")]
pub mod normalize;

pub use error::CairnError;
pub use hash::{need_hash_of_normalized, receipt_hash, NEED_DOMAIN, RECEIPT_DOMAIN};
// Normalises before hashing, so it needs the host-only Unicode tables.
#[cfg(feature = "host")]
pub use hash::need_hash;
pub use receipt::{CanonicalReceipt, LOCALE_LEN, RECEIPT_VERSION};
pub use seeds::{escrow_pda, vault_pda, ESCROW_SEED, NEED_ID_LEN, VAULT_SEED};
pub use state::{Escrow, EscrowState, ESCROW_ACCOUNT_SPACE, ESCROW_DISCRIMINATOR_PREIMAGE};

/// Smallest escrow Cairn will accept: 0.001 SOL.
///
/// Comfortably above the rent-exempt minimum for a zero-data account
/// (~0.00089 SOL), so a funded vault is never left in a state where the
/// runtime could object to its balance.
pub const MIN_ESCROW_LAMPORTS: u64 = 1_000_000;

/// A donor must give the recipient at least five minutes to respond.
pub const MIN_WINDOW_SECONDS: i64 = 300;

/// ...and at most ninety days, so nothing is locked indefinitely.
pub const MAX_WINDOW_SECONDS: i64 = 90 * 24 * 60 * 60;

/// Sentinel for "no hash yet". `receipt_hash` holds this until release.
pub const ZERO_HASH: [u8; 32] = [0u8; 32];
