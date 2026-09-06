use anchor_lang::prelude::*;

#[event]
pub struct EscrowCreated {
    pub escrow: Pubkey,
    pub donor: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub need_hash: [u8; 32],
    pub deadline: i64,
    pub created_at: i64,
}

/// Emitted on release. `receipt_hash` here is the same value written to the
/// account, so an observer tailing logs and an observer reading accounts
/// arrive at the same answer.
#[event]
pub struct ReceiptSubmitted {
    pub escrow: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub receipt_hash: [u8; 32],
    pub released_at: i64,
}

#[event]
pub struct EscrowRefunded {
    pub escrow: Pubkey,
    pub donor: Pubkey,
    pub amount: u64,
    pub refunded_at: i64,
}
