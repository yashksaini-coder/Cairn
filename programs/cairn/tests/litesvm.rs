//! In-process VM coverage of the escrow state machine.
//!
//! Every transition in §7.2 has a happy-path test, every guard in §7.3 has a
//! negative test that asserts the *specific* error code (KPI E2), and the
//! §7.4 invariants are re-checked after each transition rather than only at
//! the end.
//!
//! Requires a built program: run `anchor build` (or `cargo build-sbf`) first.

use anchor_lang::InstructionData;
use cairn_core::{
    escrow_pda, vault_pda, Escrow, EscrowState, MIN_ESCROW_LAMPORTS, MIN_WINDOW_SECONDS,
    NEED_ID_LEN, ZERO_HASH,
};
use litesvm::LiteSVM;
use solana_sdk::{
    clock::Clock,
    instruction::InstructionError,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::{Transaction, TransactionError},
};

const PROGRAM_SO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/deploy/cairn.so");
const ANCHOR_ERROR_OFFSET: u32 = 6000;
const SOL: u64 = 1_000_000_000;

// Mirrors `errors::CairnError` declaration order. Anchor numbers variants
// from 6000 in source order, so reordering that enum without touching this
// list is exactly the kind of silent break these tests exist to catch.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Code {
    AmountTooSmall = 0,
    DeadlineTooSoon = 1,
    DeadlineTooFar = 2,
    SelfEscrow = 3,
    EmptyNeedHash = 4,
    EmptyReceiptHash = 5,
    NotFunded = 6,
    NotYetExpired = 7,
    DeadlinePassed = 8,
    UnauthorizedRecipient = 9,
    UnauthorizedDonor = 10,
    EscrowStillFunded = 11,
}

struct World {
    svm: LiteSVM,
    program: Pubkey,
    donor: Keypair,
    recipient: Keypair,
    need_id: [u8; NEED_ID_LEN],
}

impl World {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let program = cairn::ID;
        svm.add_program_from_file(program, PROGRAM_SO).unwrap_or_else(|e| {
            panic!("run `anchor build` first -- could not load {PROGRAM_SO}: {e}")
        });

        // LiteSVM starts its clock at unix timestamp 0. The program stores
        // `released_at = now`, and zero is the sentinel for "not released",
        // so a 1970 clock makes a legitimately released escrow fail its own
        // invariant. Start where a real cluster could actually be.
        let mut clock = svm.get_sysvar::<Clock>();
        clock.unix_timestamp = 1_757_000_000;
        svm.set_sysvar::<Clock>(&clock);

        let donor = Keypair::new();
        let recipient = Keypair::new();
        svm.airdrop(&donor.pubkey(), 100 * SOL).unwrap();
        svm.airdrop(&recipient.pubkey(), SOL).unwrap();

        Self { svm, program, donor, recipient, need_id: [42u8; NEED_ID_LEN] }
    }

    fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    /// LiteSVM does not advance the wall clock on its own, so deadline tests
    /// move it explicitly. This is the whole reason the suite uses LiteSVM
    /// rather than a validator: waiting out a five-minute window in real time
    /// is not a test, it is a coffee break.
    fn warp(&mut self, seconds: i64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.unix_timestamp += seconds;
        self.svm.set_sysvar::<Clock>(&clock);
    }

    fn escrow(&self) -> Pubkey {
        escrow_pda(&self.program, &self.donor.pubkey(), &self.recipient.pubkey(), &self.need_id).0
    }

    fn vault(&self) -> Pubkey {
        vault_pda(&self.program, &self.escrow()).0
    }

    fn lamports(&self, k: &Pubkey) -> u64 {
        self.svm.get_account(k).map(|a| a.lamports).unwrap_or(0)
    }

    fn read_escrow(&self) -> Escrow {
        let acct = self.svm.get_account(&self.escrow()).expect("escrow account missing");
        Escrow::try_decode(&acct.data).expect("escrow failed to decode via cairn-core")
    }

    /// Invariants 1-3 and 7 (§7.4), re-asserted after every transition.
    fn assert_invariants(&self) {
        let e = self.read_escrow();
        assert!(
            e.invariants_hold(self.lamports(&self.vault())),
            "invariant violated: {e:?} with vault balance {}",
            self.lamports(&self.vault())
        );
    }

    fn send(&mut self, ix: Instruction, signers: &[&Keypair]) -> Result<(), TransactionError> {
        // A real client fetches a fresh blockhash per call. Without this, two
        // identical instructions from the same signer produce a byte-identical
        // transaction, and the runtime rejects the second as a duplicate
        // signature *before the program runs* -- so a test asserting that a
        // terminal state refuses a second attempt would pass for entirely the
        // wrong reason, or, as here, fail with AlreadyProcessed.
        self.svm.expire_blockhash();

        let payer = signers[0].pubkey();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer),
            signers,
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx).map(|_| ()).map_err(|e| e.err)
    }

    fn create_ix(&self, amount: u64, need_hash: [u8; 32], deadline: i64) -> Instruction {
        Instruction {
            program_id: self.program,
            accounts: vec![
                AccountMeta::new(self.donor.pubkey(), true),
                AccountMeta::new_readonly(self.recipient.pubkey(), false),
                AccountMeta::new(self.escrow(), false),
                AccountMeta::new(self.vault(), false),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            ],
            data: cairn::instruction::CreateEscrow {
                need_id: self.need_id,
                amount,
                need_hash,
                deadline,
            }
            .data(),
        }
    }

    fn create(&mut self, amount: u64, window: i64) -> Result<(), TransactionError> {
        let deadline = self.now() + window;
        let ix = self.create_ix(amount, [7u8; 32], deadline);
        let donor = self.donor.insecure_clone();
        self.send(ix, &[&donor])
    }

    fn submit_ix(&self, signer: &Pubkey, receipt_hash: [u8; 32]) -> Instruction {
        Instruction {
            program_id: self.program,
            accounts: vec![
                AccountMeta::new(self.escrow(), false),
                AccountMeta::new(self.vault(), false),
                AccountMeta::new(*signer, true),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            ],
            data: cairn::instruction::SubmitReceipt { receipt_hash }.data(),
        }
    }

    fn submit(&mut self, receipt_hash: [u8; 32]) -> Result<(), TransactionError> {
        let r = self.recipient.insecure_clone();
        let ix = self.submit_ix(&r.pubkey(), receipt_hash);
        self.send(ix, &[&r])
    }

    fn refund_ix(&self, signer: &Pubkey) -> Instruction {
        Instruction {
            program_id: self.program,
            accounts: vec![
                AccountMeta::new(self.escrow(), false),
                AccountMeta::new(self.vault(), false),
                AccountMeta::new(*signer, true),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            ],
            data: cairn::instruction::Refund {}.data(),
        }
    }

    fn refund(&mut self) -> Result<(), TransactionError> {
        let d = self.donor.insecure_clone();
        let ix = self.refund_ix(&d.pubkey());
        self.send(ix, &[&d])
    }
}

#[track_caller]
fn assert_cairn_error(res: Result<(), TransactionError>, expected: Code) {
    let code = ANCHOR_ERROR_OFFSET + expected as u32;
    match res {
        Err(TransactionError::InstructionError(_, InstructionError::Custom(got))) => {
            assert_eq!(got, code, "wrong rejection reason: got {got}, wanted {code}");
        }
        other => panic!("expected custom error {code}, got {other:?}"),
    }
}

// ---------------------------------------------------------------- happy paths

#[test]
fn funded_to_released() {
    let mut w = World::new();
    let amount = 2 * SOL;
    w.create(amount, 3600).unwrap();
    w.assert_invariants();
    assert_eq!(w.lamports(&w.vault()), amount, "vault balance must match the escrow amount (F1)");

    let before = w.lamports(&w.recipient.pubkey());
    let receipt = [9u8; 32];
    w.submit(receipt).unwrap();
    w.assert_invariants();

    let e = w.read_escrow();
    assert_eq!(e.state, EscrowState::Released);
    assert_eq!(e.receipt_hash, receipt);
    assert_ne!(e.released_at, 0);
    assert_eq!(w.lamports(&w.vault()), 0, "vault must be drained on release");

    // Invariant 4: no lamport is created. The recipient pays the fee, so they
    // net slightly less than `amount`, never more.
    let gained = w.lamports(&w.recipient.pubkey()) - before;
    assert!(gained <= amount && gained > amount - 100_000, "recipient gained {gained}");
}

#[test]
fn funded_to_refunded() {
    let mut w = World::new();
    let amount = 3 * SOL;
    w.create(amount, MIN_WINDOW_SECONDS).unwrap();
    let before = w.lamports(&w.donor.pubkey());

    w.warp(MIN_WINDOW_SECONDS + 1);
    w.refund().unwrap();
    w.assert_invariants();

    let e = w.read_escrow();
    assert_eq!(e.state, EscrowState::Refunded);
    assert_eq!(e.receipt_hash, ZERO_HASH, "a refund must not fabricate a receipt");
    assert_eq!(e.released_at, 0);
    assert!(w.lamports(&w.donor.pubkey()) > before, "donor got the money back");
    assert_eq!(w.lamports(&w.vault()), 0);
}

#[test]
fn release_is_valid_right_up_to_the_deadline() {
    let mut w = World::new();
    w.create(SOL, MIN_WINDOW_SECONDS).unwrap();
    w.warp(MIN_WINDOW_SECONDS - 1);
    w.submit([1u8; 32]).unwrap();
    assert_eq!(w.read_escrow().state, EscrowState::Released);
}

// ------------------------------------------------------- creation guards §7.3

#[test]
fn rejects_dust() {
    let mut w = World::new();
    assert_cairn_error(w.create(MIN_ESCROW_LAMPORTS - 1, 3600), Code::AmountTooSmall);
}

#[test]
fn rejects_a_deadline_inside_the_minimum_window() {
    let mut w = World::new();
    assert_cairn_error(w.create(SOL, MIN_WINDOW_SECONDS - 1), Code::DeadlineTooSoon);
}

#[test]
fn rejects_a_deadline_past_ninety_days() {
    let mut w = World::new();
    assert_cairn_error(w.create(SOL, cairn_core::MAX_WINDOW_SECONDS + 60), Code::DeadlineTooFar);
}

#[test]
fn rejects_a_zero_need_hash() {
    let mut w = World::new();
    let deadline = w.now() + 3600;
    let ix = w.create_ix(SOL, ZERO_HASH, deadline);
    let donor = w.donor.insecure_clone();
    assert_cairn_error(w.send(ix, &[&donor]), Code::EmptyNeedHash);
}

#[test]
fn rejects_self_dealing() {
    let mut w = World::new();
    w.recipient = w.donor.insecure_clone();
    assert_cairn_error(w.create(SOL, 3600), Code::SelfEscrow);
}

// -------------------------------------------------------- release guards §7.3

#[test]
fn only_the_named_recipient_can_release() {
    // Invariant 5. This is the single most important negative test in the
    // suite: if it ever passes for an impostor, Cairn has no trust model.
    let mut w = World::new();
    w.create(5 * SOL, 3600).unwrap();

    let impostor = Keypair::new();
    w.svm.airdrop(&impostor.pubkey(), SOL).unwrap();
    let ix = w.submit_ix(&impostor.pubkey(), [3u8; 32]);
    assert_cairn_error(w.send(ix, &[&impostor]), Code::UnauthorizedRecipient);

    // ...and the donor is not privileged either.
    let donor = w.donor.insecure_clone();
    let ix = w.submit_ix(&donor.pubkey(), [3u8; 32]);
    assert_cairn_error(w.send(ix, &[&donor]), Code::UnauthorizedRecipient);

    w.assert_invariants();
    assert_eq!(w.lamports(&w.vault()), 5 * SOL, "vault untouched by failed releases");
}

#[test]
fn rejects_release_after_the_deadline() {
    let mut w = World::new();
    w.create(SOL, MIN_WINDOW_SECONDS).unwrap();
    w.warp(MIN_WINDOW_SECONDS);
    assert_cairn_error(w.submit([1u8; 32]), Code::DeadlinePassed);
}

#[test]
fn rejects_a_zero_receipt_hash() {
    let mut w = World::new();
    w.create(SOL, 3600).unwrap();
    assert_cairn_error(w.submit(ZERO_HASH), Code::EmptyReceiptHash);
}

#[test]
fn released_is_terminal() {
    let mut w = World::new();
    w.create(SOL, 3600).unwrap();
    w.submit([1u8; 32]).unwrap();
    assert_cairn_error(w.submit([2u8; 32]), Code::NotFunded);
    assert_eq!(w.read_escrow().receipt_hash, [1u8; 32], "first receipt stands");

    w.warp(4000);
    assert_cairn_error(w.refund(), Code::NotFunded);
}

// --------------------------------------------------------- refund guards §7.3

#[test]
fn rejects_refund_before_the_deadline() {
    let mut w = World::new();
    w.create(SOL, 3600).unwrap();
    assert_cairn_error(w.refund(), Code::NotYetExpired);
    w.assert_invariants();
}

#[test]
fn only_the_donor_can_refund() {
    // Invariant 6.
    let mut w = World::new();
    w.create(SOL, MIN_WINDOW_SECONDS).unwrap();
    w.warp(MIN_WINDOW_SECONDS + 1);

    let r = w.recipient.insecure_clone();
    let ix = w.refund_ix(&r.pubkey());
    assert_cairn_error(w.send(ix, &[&r]), Code::UnauthorizedDonor);
    w.assert_invariants();
}

#[test]
fn refunded_is_terminal() {
    let mut w = World::new();
    w.create(SOL, MIN_WINDOW_SECONDS).unwrap();
    w.warp(MIN_WINDOW_SECONDS + 1);
    w.refund().unwrap();
    assert_cairn_error(w.refund(), Code::NotFunded);
    assert_cairn_error(w.submit([1u8; 32]), Code::NotFunded);
}

// ------------------------------------------------------------ immutability §7

#[test]
fn creation_terms_are_immutable() {
    // Invariant 7: nothing in the program can rewrite the terms, so the only
    // way to check this is that re-running creation over a live escrow fails
    // rather than overwriting it.
    let mut w = World::new();
    w.create(SOL, 3600).unwrap();
    let before = w.read_escrow();

    assert!(w.create(50 * SOL, 7200).is_err(), "re-initialising an escrow must fail");
    assert_eq!(w.read_escrow(), before);
}

#[test]
fn close_reclaims_rent_only_after_settlement() {
    let mut w = World::new();
    w.create(SOL, 3600).unwrap();

    let escrow_key = w.escrow();
    let close = move |signer: &Pubkey| Instruction {
        program_id: cairn::ID,
        accounts: vec![AccountMeta::new(escrow_key, false), AccountMeta::new(*signer, true)],
        data: cairn::instruction::CloseEscrow {}.data(),
    };

    let donor = w.donor.insecure_clone();
    assert_cairn_error(w.send(close(&donor.pubkey()), &[&donor]), Code::EscrowStillFunded);

    w.submit([1u8; 32]).unwrap();
    let before = w.lamports(&donor.pubkey());
    w.send(close(&donor.pubkey()), &[&donor]).unwrap();
    assert!(w.lamports(&donor.pubkey()) > before, "rent returned to donor");
    assert!(w.svm.get_account(&escrow_key).map(|a| a.data.is_empty()).unwrap_or(true));
}

#[test]
fn discriminator_survives_a_round_trip_through_the_chain() {
    // F4 in miniature: the bytes the program writes are the bytes
    // `cairn-core` reads. `read_escrow` decodes with the shared crate, so a
    // layout drift between program and indexer fails right here.
    let mut w = World::new();
    w.create(SOL, 3600).unwrap();
    let e = w.read_escrow();
    assert_eq!(e.donor, w.donor.pubkey());
    assert_eq!(e.recipient, w.recipient.pubkey());
    assert_eq!(e.amount, SOL);
    assert_eq!(escrow_pda(&w.program, &e.donor, &e.recipient, &w.need_id).1, e.bump);
    assert_eq!(vault_pda(&w.program, &w.escrow()).1, e.vault_bump);
}
