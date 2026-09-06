use anchor_lang::prelude::*;

use crate::errors::CairnError;
use crate::state::{Escrow, EscrowState};

/// P2 (§11 F17). Reclaims the escrow account's rent once it has settled.
///
/// Closing destroys the on-chain record, so the receipt hash survives only in
/// the release transaction and its logs. That is a real trade-off and the UI
/// should say so before offering the action.
#[derive(Accounts)]
pub struct CloseEscrow<'info> {
    #[account(
        mut,
        close = donor,
        constraint = escrow.donor == donor.key() @ CairnError::UnauthorizedDonor,
        constraint = escrow.state != EscrowState::Funded @ CairnError::EscrowStillFunded,
    )]
    pub escrow: Account<'info, Escrow>,

    #[account(mut)]
    pub donor: Signer<'info>,
}

pub fn handle_close_escrow(_ctx: Context<CloseEscrow>) -> Result<()> {
    Ok(())
}
