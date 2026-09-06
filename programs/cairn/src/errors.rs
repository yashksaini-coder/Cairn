use anchor_lang::prelude::*;

/// Each guard in §7.3 gets its own code, so the negative tests can assert
/// *why* a transaction was rejected rather than just that it was (KPI E2).
#[error_code]
pub enum CairnError {
    #[msg("Escrow amount is below the 0.001 SOL minimum")]
    AmountTooSmall,
    #[msg("Deadline must be at least 5 minutes from now")]
    DeadlineTooSoon,
    #[msg("Deadline must be within 90 days")]
    DeadlineTooFar,
    #[msg("A donor cannot be their own recipient")]
    SelfEscrow,
    #[msg("Need hash must not be zero")]
    EmptyNeedHash,
    #[msg("Receipt hash must not be zero")]
    EmptyReceiptHash,
    #[msg("Escrow is not in the Funded state")]
    NotFunded,
    #[msg("Escrow is still open; refunds are only possible after the deadline")]
    NotYetExpired,
    #[msg("Escrow deadline has passed; the donor may now refund")]
    DeadlinePassed,
    #[msg("Signer is not the recipient named on this escrow")]
    UnauthorizedRecipient,
    #[msg("Signer is not the donor who created this escrow")]
    UnauthorizedDonor,
    #[msg("Escrow is still holding funds and cannot be closed")]
    EscrowStillFunded,
    #[msg("Vault balance does not match the escrow amount")]
    VaultBalanceMismatch,
}
