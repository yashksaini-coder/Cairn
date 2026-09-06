use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::errors::CairnError;
use crate::events::EscrowCreated;
use crate::state::{Escrow, EscrowData, EscrowState, ESCROW_ACCOUNT_SPACE};
use cairn_core::{
    ESCROW_SEED, MAX_WINDOW_SECONDS, MIN_ESCROW_LAMPORTS, MIN_WINDOW_SECONDS, NEED_ID_LEN,
    VAULT_SEED, ZERO_HASH,
};

#[derive(Accounts)]
#[instruction(need_id: [u8; NEED_ID_LEN])]
pub struct CreateEscrow<'info> {
    #[account(mut)]
    pub donor: Signer<'info>,

    /// The recipient does not sign here -- the donor names them, and the
    /// account is validated only as an ordinary system-owned wallet. It may
    /// not exist yet; an address that has never been used still reports the
    /// system program as its owner, so a brand-new recipient wallet works.
    pub recipient: SystemAccount<'info>,

    #[account(
        init,
        payer = donor,
        space = ESCROW_ACCOUNT_SPACE,
        seeds = [ESCROW_SEED, donor.key().as_ref(), recipient.key().as_ref(), need_id.as_ref()],
        bump,
    )]
    pub escrow: Account<'info, Escrow>,

    /// Dataless lamport holder. Never allocated, so there is no rent to
    /// reclaim later and draining it to zero simply reaps the account.
    #[account(
        mut,
        seeds = [VAULT_SEED, escrow.key().as_ref()],
        bump,
    )]
    pub vault: SystemAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handle_create_escrow(
    ctx: Context<CreateEscrow>,
    _need_id: [u8; NEED_ID_LEN],
    amount: u64,
    need_hash: [u8; 32],
    deadline: i64,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    require!(amount >= MIN_ESCROW_LAMPORTS, CairnError::AmountTooSmall);
    require!(deadline >= now + MIN_WINDOW_SECONDS, CairnError::DeadlineTooSoon);
    require!(deadline <= now + MAX_WINDOW_SECONDS, CairnError::DeadlineTooFar);
    require!(need_hash != ZERO_HASH, CairnError::EmptyNeedHash);
    require_keys_neq!(
        ctx.accounts.donor.key(),
        ctx.accounts.recipient.key(),
        CairnError::SelfEscrow
    );

    // Funds move before state is written. If the transfer fails the whole
    // instruction unwinds, so there is no window in which an escrow reads
    // Funded over an empty vault (invariant 1).
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.key(),
            system_program::Transfer {
                from: ctx.accounts.donor.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
            },
        ),
        amount,
    )?;

    ctx.accounts.escrow.set_inner(Escrow {
        inner: EscrowData {
            donor: ctx.accounts.donor.key(),
            recipient: ctx.accounts.recipient.key(),
            amount,
            need_hash,
            receipt_hash: ZERO_HASH,
            deadline,
            state: EscrowState::Funded,
            created_at: now,
            released_at: 0,
            bump: ctx.bumps.escrow,
            vault_bump: ctx.bumps.vault,
        },
    });

    emit!(EscrowCreated {
        escrow: ctx.accounts.escrow.key(),
        donor: ctx.accounts.donor.key(),
        recipient: ctx.accounts.recipient.key(),
        amount,
        need_hash,
        deadline,
        created_at: now,
    });

    Ok(())
}
