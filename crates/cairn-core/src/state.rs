//! Escrow account layout and the state machine.
//!
//! The struct lives here rather than in the program so that the indexer
//! decodes exactly what the program wrote. `programs/cairn/src/state.rs`
//! implements Anchor's account traits over this type; there is no second
//! field list to keep in sync.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::Pubkey;

/// Anchor derives an account discriminator as `sha256("account:<Name>")[..8]`.
/// Kept as bytes so no syscall runs on every deserialize; the test below
/// proves the constant is right.
pub const ESCROW_DISCRIMINATOR: [u8; 8] = [31, 213, 123, 187, 186, 22, 218, 155];
pub const ESCROW_DISCRIMINATOR_PREIMAGE: &[u8] = b"account:Escrow";

/// 8 discriminator + 163 payload = 171 used. Allocated at 200 to leave room
/// for the P2 fields (§11) without a migration.
pub const ESCROW_ACCOUNT_SPACE: usize = 200;

/// Lifecycle of an escrow. `Released` and `Refunded` are terminal: the
/// program contains no instruction that transitions out of either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "host", derive(serde::Serialize, serde::Deserialize))]
// borsh 1.x refuses to guess for enums with explicit discriminants, and it is
// right to: the values below are the on-chain encoding, so writing them is a
// deliberate choice rather than an accident of declaration order. They happen
// to coincide with the index order Anchor would have produced anyway.
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum EscrowState {
    Funded = 0,
    Released = 1,
    Refunded = 2,
}

impl EscrowState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, EscrowState::Funded)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EscrowState::Funded => "funded",
            EscrowState::Released => "released",
            EscrowState::Refunded => "refunded",
        }
    }
}

/// The inverse of [`EscrowState::as_str`], as the standard trait rather than
/// an inherent `from_str` that shadows it.
impl core::str::FromStr for EscrowState {
    type Err = crate::CairnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "funded" => Ok(EscrowState::Funded),
            "released" => Ok(EscrowState::Released),
            "refunded" => Ok(EscrowState::Refunded),
            _ => Err(crate::CairnError::UnknownState),
        }
    }
}

/// The escrow record. Field order is the borsh wire order -- do not reorder.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "host", derive(serde::Serialize, serde::Deserialize))]
pub struct Escrow {
    pub donor: Pubkey,
    /// Named by the donor at creation and immutable thereafter. This single
    /// field is the whole authorization model: only a signature from this
    /// key can release the funds.
    pub recipient: Pubkey,
    pub amount: u64,
    pub need_hash: [u8; 32],
    /// Zero until release. Non-zero if and only if `state == Released`.
    pub receipt_hash: [u8; 32],
    pub deadline: i64,
    pub state: EscrowState,
    pub created_at: i64,
    /// Zero until release.
    pub released_at: i64,
    pub bump: u8,
    pub vault_bump: u8,
}

impl Escrow {
    /// Decode an on-chain account, discriminator included.
    ///
    /// Used by the indexer and the verifier. Trailing bytes are expected --
    /// the account is over-allocated -- so this deliberately does not use
    /// borsh's exact-length `try_from_slice`.
    pub fn try_decode(data: &[u8]) -> Result<Self, crate::CairnError> {
        let body = data
            .get(8..)
            .filter(|_| data[..8] == ESCROW_DISCRIMINATOR)
            .ok_or(crate::CairnError::BadDiscriminator)?;
        let mut cursor = body;
        Escrow::deserialize(&mut cursor).map_err(|_| crate::CairnError::BadAccountData)
    }

    /// Invariants 1-3 and 7 from §7.4, checkable from a decoded account alone.
    ///
    /// Asserted after every transition in the LiteSVM suite and by the CLI's
    /// backfill, which is the cheapest possible smoke test that the program
    /// and the indexer agree.
    pub fn invariants_hold(&self, vault_lamports: u64) -> bool {
        let funded = self.state == EscrowState::Funded;
        let released = self.state == EscrowState::Released;
        (vault_lamports > 0) == funded
            && (self.receipt_hash != crate::ZERO_HASH) == released
            && (self.released_at != 0) == released
            && self.donor != self.recipient
            && self.need_hash != crate::ZERO_HASH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::hash::hash;

    #[test]
    fn discriminator_matches_anchor() {
        assert_eq!(&hash(ESCROW_DISCRIMINATOR_PREIMAGE).to_bytes()[..8], &ESCROW_DISCRIMINATOR);
    }

    fn sample(state: EscrowState) -> Escrow {
        Escrow {
            donor: Pubkey::new_from_array([1u8; 32]),
            recipient: Pubkey::new_from_array([2u8; 32]),
            amount: 1_000_000,
            need_hash: [3u8; 32],
            receipt_hash: crate::ZERO_HASH,
            deadline: 1_757_000_000,
            state,
            created_at: 1_756_000_000,
            released_at: 0,
            bump: 254,
            vault_bump: 253,
        }
    }

    #[test]
    fn payload_fits_allocation() {
        let bytes = borsh::to_vec(&sample(EscrowState::Funded)).unwrap();
        assert_eq!(bytes.len(), 163);
        assert!(8 + bytes.len() <= ESCROW_ACCOUNT_SPACE);
    }

    #[test]
    fn round_trips_over_an_overallocated_account() {
        let e = sample(EscrowState::Funded);
        let mut raw = ESCROW_DISCRIMINATOR.to_vec();
        raw.extend(borsh::to_vec(&e).unwrap());
        raw.resize(ESCROW_ACCOUNT_SPACE, 0); // the zero padding a real account carries
        assert_eq!(Escrow::try_decode(&raw).unwrap(), e);
    }

    #[test]
    fn rejects_a_foreign_account() {
        let raw = [0u8; ESCROW_ACCOUNT_SPACE];
        assert!(Escrow::try_decode(&raw).is_err());
        assert!(Escrow::try_decode(&[]).is_err());
    }

    #[test]
    fn invariants_catch_an_impossible_record() {
        let funded = sample(EscrowState::Funded);
        assert!(funded.invariants_hold(1_000_000));
        assert!(!funded.invariants_hold(0), "funded escrow with an empty vault");

        let mut half_released = sample(EscrowState::Released);
        half_released.receipt_hash = [4u8; 32];
        assert!(!half_released.invariants_hold(0), "released without a timestamp");
        half_released.released_at = 1_756_500_000;
        assert!(half_released.invariants_hold(0));
        assert!(!half_released.invariants_hold(5), "released but the vault still holds funds");
    }
}
