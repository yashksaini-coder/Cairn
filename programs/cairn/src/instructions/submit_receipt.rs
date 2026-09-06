use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::errors::CairnError;
use crate::events::ReceiptSubmitted;
use crate::state::{Escrow, EscrowState};
use cairn_core::{VAULT_SEED, ZERO_HASH};

/// Account order is part of the wire format: the verifier (§8.4) reads the
/// escrow pubkey out of a confirmed transaction by index, so inserting an
/// account above `escrow` breaks verification of every past receipt.
#[derive(Accounts)]
pub struct SubmitReceipt<'info> {
    /// Not re-derived from seeds. `need_id` is a creation-time nonce that is
    /// never stored, and re-deriving would buy nothing: `Account<'info, _>`
    /// already proves this program owns the account and wrote the
    /// discriminator, and the constraint below proves the signer is the
    /// recipient recorded *inside* it. The account's contents are the
    /// authority after creation, not its address.
    #[account(
        mut,
        constraint = escrow.recipient == recipient.key() @ CairnError::UnauthorizedRecipient,
    )]
    pub escrow: Account<'info, Escrow>,

    #[account(
        mut,
        seeds = [VAULT_SEED, escrow.key().as_ref()],
        bump = escrow.vault_bump,
    )]
    pub vault: SystemAccount<'info>,

    /// The whole authorization model, in one line.
    #[account(mut)]
    pub recipient: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_submit_receipt(ctx: Context<SubmitReceipt>, receipt_hash: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let escrow = &mut ctx.accounts.escrow;

    require!(escrow.state == EscrowState::Funded, CairnError::NotFunded);
    require!(now < escrow.deadline, CairnError::DeadlinePassed);
    require!(receipt_hash != ZERO_HASH, CairnError::EmptyReceiptHash);

    // Pay out whatever is actually in the vault rather than the recorded
    // amount. They are equal in every path this program can produce, but an
    // unsolicited transfer into the vault would otherwise be stranded there
    // forever, leaving a non-empty vault on a Released escrow (invariant 1).
    let payout = ctx.accounts.vault.lamports();
    require!(payout >= escrow.amount, CairnError::VaultBalanceMismatch);

    escrow.receipt_hash = receipt_hash;
    escrow.state = EscrowState::Released;
    escrow.released_at = now;

    let escrow_key = escrow.key();
    let vault_bump = escrow.vault_bump;
    let amount = escrow.amount;
    let vault_seeds: &[&[u8]] = &[VAULT_SEED, escrow_key.as_ref(), &[vault_bump]];

    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.key(),
            system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.recipient.to_account_info(),
            },
            &[vault_seeds],
        ),
        payout,
    )?;

    emit!(ReceiptSubmitted {
        escrow: escrow_key,
        recipient: ctx.accounts.recipient.key(),
        amount,
        receipt_hash,
        released_at: now,
    });

    Ok(())
}
