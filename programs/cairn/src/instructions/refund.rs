use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::errors::CairnError;
use crate::events::EscrowRefunded;
use crate::state::{Escrow, EscrowState};
use cairn_core::VAULT_SEED;

#[derive(Accounts)]
pub struct Refund<'info> {
    #[account(
        mut,
        constraint = escrow.donor == donor.key() @ CairnError::UnauthorizedDonor,
    )]
    pub escrow: Account<'info, Escrow>,

    #[account(
        mut,
        seeds = [VAULT_SEED, escrow.key().as_ref()],
        bump = escrow.vault_bump,
    )]
    pub vault: SystemAccount<'info>,

    #[account(mut)]
    pub donor: Signer<'info>,

    pub system_program: Program<'info, System>,
}

/// The refund path exists so that nothing is ever stranded (§2, "funds
/// stranded when nothing happens"). Note what it is *not*: there is no
/// operator key, no platform escape hatch and no admin instruction. If the
/// recipient never records a receipt, the only account that can recover the
/// money is the one that put it in, and only after the window it chose.
pub fn handle_refund(ctx: Context<Refund>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let escrow = &mut ctx.accounts.escrow;

    require!(escrow.state == EscrowState::Funded, CairnError::NotFunded);
    require!(now >= escrow.deadline, CairnError::NotYetExpired);

    let payout = ctx.accounts.vault.lamports();
    escrow.state = EscrowState::Refunded;

    let escrow_key = escrow.key();
    let vault_bump = escrow.vault_bump;
    let amount = escrow.amount;
    let vault_seeds: &[&[u8]] = &[VAULT_SEED, escrow_key.as_ref(), &[vault_bump]];

    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.key(),
            system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.donor.to_account_info(),
            },
            &[vault_seeds],
        ),
        payout,
    )?;

    emit!(EscrowRefunded {
        escrow: escrow_key,
        donor: ctx.accounts.donor.key(),
        amount,
        refunded_at: now,
    });

    Ok(())
}
