//! PDA seeds. Shared so the browser, the server and the program all derive
//! the same addresses.

use solana_program::pubkey::Pubkey;

pub const ESCROW_SEED: &[u8] = b"escrow";
pub const VAULT_SEED: &[u8] = b"vault";

/// Client-chosen nonce that lets one donor open several escrows to the same
/// recipient. Not stored on the account -- it exists only to disambiguate the
/// PDA, and the escrow address itself is the identifier everywhere else.
pub const NEED_ID_LEN: usize = 16;

/// `["escrow", donor, recipient, need_id]`
pub fn escrow_pda(
    program_id: &Pubkey,
    donor: &Pubkey,
    recipient: &Pubkey,
    need_id: &[u8; NEED_ID_LEN],
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[ESCROW_SEED, donor.as_ref(), recipient.as_ref(), need_id],
        program_id,
    )
}

/// `["vault", escrow]`
///
/// System-owned and dataless: it is a lamport holder, never an allocated
/// account. That is why release and refund can drain it to zero and let the
/// runtime reap it, with no rent to reclaim and no `close` to get wrong.
pub fn vault_pda(program_id: &Pubkey, escrow: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, escrow.as_ref()], program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_separate_needs_by_the_same_pair() {
        let program = Pubkey::new_unique();
        let (donor, recipient) = (Pubkey::new_unique(), Pubkey::new_unique());
        let a = escrow_pda(&program, &donor, &recipient, &[0u8; NEED_ID_LEN]).0;
        let b = escrow_pda(&program, &donor, &recipient, &[1u8; NEED_ID_LEN]).0;
        assert_ne!(a, b);
        assert_ne!(vault_pda(&program, &a).0, vault_pda(&program, &b).0);
    }

    #[test]
    fn direction_matters() {
        // Swapping donor and recipient must not land on the same escrow.
        let program = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let id = [7u8; NEED_ID_LEN];
        assert_ne!(escrow_pda(&program, &a, &b, &id).0, escrow_pda(&program, &b, &a, &id).0);
    }
}
