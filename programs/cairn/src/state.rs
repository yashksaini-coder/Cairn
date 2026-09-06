//! Anchor's view of the escrow account.
//!
//! The field list lives in `cairn-core`. This file exists only to dress that
//! struct in the traits Anchor's `Account<'info, _>` wrapper needs.
//!
//! It has to be a newtype rather than a direct `impl` block: `Discriminator`
//! and friends are foreign traits and `cairn_core::Escrow` is a foreign type,
//! so the orphan rule forbids implementing them here. Wrapping a single
//! borsh-serialisable field costs nothing on the wire -- borsh flattens it --
//! so the bytes this program writes are byte-for-byte what
//! `cairn_core::Escrow::try_decode` reads back in the indexer.
//!
//! The struct is named `Escrow` deliberately. Anchor derives the account
//! discriminator from the type name, and `sha256("account:Escrow")[..8]` is
//! the constant `cairn-core` hardcodes. Rename it and the indexer goes blind;
//! `discriminator_matches_core` below is the tripwire.

use anchor_lang::prelude::*;
use core::ops::{Deref, DerefMut};

pub use cairn_core::state::{EscrowState, ESCROW_ACCOUNT_SPACE};
pub type EscrowData = cairn_core::state::Escrow;

#[account]
#[derive(Debug)]
pub struct Escrow {
    pub inner: EscrowData,
}

impl Deref for Escrow {
    type Target = EscrowData;
    fn deref(&self) -> &EscrowData {
        &self.inner
    }
}

impl DerefMut for Escrow {
    fn deref_mut(&mut self) -> &mut EscrowData {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::Discriminator;

    #[test]
    fn discriminator_matches_core() {
        assert_eq!(
            Escrow::DISCRIMINATOR,
            &cairn_core::state::ESCROW_DISCRIMINATOR[..],
            "program and indexer disagree about what a Cairn escrow account looks like"
        );
    }
}
