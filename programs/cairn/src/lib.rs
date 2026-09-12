//! # Cairn
//!
//! An escrow whose release is gated on a signature from the recipient's own
//! wallet, and whose release transaction carries the hash of a voice
//! recording in which the recipient states what they received.
//!
//! Two things are deliberately kept apart here, and the distinction governs
//! the whole design:
//!
//! * **Authorization** is the ed25519 signature checked below. It is the only
//!   thing that moves money, and nothing off-chain can substitute for it.
//! * **Testimony** is the recording, present only as `receipt_hash`. It is
//!   evidence bound to the transfer, *not* an authentication factor. A voice
//!   can be replayed or synthesised; a signature cannot. Nothing in this
//!   program treats the hash as permission.
//!
//! The program stores no audio, no transcript and no need text. It stores
//! commitments to them.

use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

pub use errors::CairnError;
pub use state::{Escrow, EscrowState};

pub use instructions::*;

// The address this program asserts it lives at. The matching keypair is in
// .demo/, which is gitignored, so this committed value is not one you can
// deploy to -- `just sync-id` rewrites it from your own keypair, and
// `just deploy` refuses to ship a binary whose declared id does not match
// the key it is deploying with.
declare_id!("Gt2Ki3qNfrVMzauSJNjpHf5YUck3f9rQonkfJ6sSTNiR");

#[program]
pub mod cairn {
    use super::*;

    /// Lock `amount` lamports for `recipient`, redeemable until `deadline`.
    pub fn create_escrow(
        ctx: Context<CreateEscrow>,
        need_id: [u8; cairn_core::NEED_ID_LEN],
        amount: u64,
        need_hash: [u8; 32],
        deadline: i64,
    ) -> Result<()> {
        instructions::create_escrow::handle_create_escrow(ctx, need_id, amount, need_hash, deadline)
    }

    /// Commit a receipt hash and release the funds. Recipient signs.
    ///
    /// This is the only path out of `Funded` into `Released`, and it writes
    /// the hash and moves the money in the same instruction -- so a receipt
    /// cannot be attached to a transfer after the fact, and a transfer cannot
    /// happen without one.
    pub fn submit_receipt(ctx: Context<SubmitReceipt>, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::submit_receipt::handle_submit_receipt(ctx, receipt_hash)
    }

    /// Return an expired escrow's funds to the donor. Donor signs.
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        instructions::refund::handle_refund(ctx)
    }

    /// Reclaim rent from a settled escrow. Donor signs. (P2, §11 F17.)
    pub fn close_escrow(ctx: Context<CloseEscrow>) -> Result<()> {
        instructions::close_escrow::handle_close_escrow(ctx)
    }
}
