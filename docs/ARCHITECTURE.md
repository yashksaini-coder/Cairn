# Architecture notes

Reference material that would bloat the README. The trust model lives there;
this is the mechanical detail.

## On-chain

### Accounts

**Escrow** — PDA, seeds `["escrow", donor, recipient, need_id]`, 200 bytes
allocated (8 discriminator + 163 payload; the slack is headroom for the
deferred P2 fields).

| Field | Type | Notes |
|---|---|---|
| `donor` | `Pubkey` | |
| `recipient` | `Pubkey` | Fixed at creation, immutable |
| `amount` | `u64` | Lamports |
| `need_hash` | `[u8; 32]` | |
| `receipt_hash` | `[u8; 32]` | Zeroed until release |
| `deadline` | `i64` | Unix seconds |
| `state` | `EscrowState` | `Funded = 0`, `Released = 1`, `Refunded = 2` |
| `created_at` | `i64` | |
| `released_at` | `i64` | Zero until release |
| `bump`, `vault_bump` | `u8` | |

`need_id` is not stored. It is a creation-time nonce that only disambiguates
the PDA, and after creation the escrow's *contents* are the authority, not its
address — which is why `submit_receipt` and `refund` do not re-derive seeds.

**Vault** — PDA, seeds `["vault", escrow]`. System-owned, dataless. It is
never allocated, so there is no rent to reclaim and draining it to zero simply
lets the runtime reap it.

### State machine

```
                submit_receipt (recipient signature, before the deadline)
        ┌────────────────────────────────────────────────────► Released
        │
     Funded
        │
        └────────────────────────────────────────────────────► Refunded
                refund (donor signature, at or after the deadline)
```

Both end states are terminal. No instruction transitions out of either.

### Invariants

Asserted in `programs/cairn/tests/litesvm.rs` after every transition, not only
at the end, and re-checked by `cairn inspect` against live devnet state.

1. The vault holds lamports if and only if the state is `Funded`.
2. `receipt_hash` is non-zero if and only if the state is `Released`.
3. `released_at` is non-zero if and only if the state is `Released`.
4. Lamports out never exceed lamports in.
5. No path lets anyone but `escrow.recipient` trigger a release.
6. No path lets anyone but `escrow.donor` trigger a refund.
7. `recipient`, `need_hash`, `amount` and `deadline` are immutable after
   creation.

Invariants 5 and 6 are the ones that matter. `only_the_named_recipient_can_release`
is the single most important negative test in the suite: if it ever passes for
an impostor, Cairn has no trust model.

### Guards, and their error codes

Anchor numbers custom errors from 6000 in declaration order. Each guard has
its own code so tests assert *why* a transaction was rejected, not merely that
it was.

| Code | Error | Instruction |
|---|---|---|
| 6000 | `AmountTooSmall` | create |
| 6001 | `DeadlineTooSoon` | create |
| 6002 | `DeadlineTooFar` | create |
| 6003 | `SelfEscrow` | create |
| 6004 | `EmptyNeedHash` | create |
| 6005 | `EmptyReceiptHash` | submit |
| 6006 | `NotFunded` | submit, refund |
| 6007 | `NotYetExpired` | refund |
| 6008 | `DeadlinePassed` | submit |
| 6009 | `UnauthorizedRecipient` | submit |
| 6010 | `UnauthorizedDonor` | refund, close |
| 6011 | `EscrowStillFunded` | close |
| 6012 | `VaultBalanceMismatch` | submit |

Reordering `errors::CairnError` renumbers everything, and the `Code` enum in
the LiteSVM suite mirrors this table by position -- which is the point: a
reorder without a matching edit there turns every negative test into a test of
the wrong rejection reason.

## API

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `POST` | `/v1/receipts/challenge` | none | Issue a nonce bound to one escrow |
| `POST` | `/v1/receipts` | wallet challenge signature | Upload audio, transcribe, return `receipt_hash` |
| `GET` | `/v1/receipts/{hash}` | none | Transcript, locale, audio URL, timestamp |
| `GET` | `/v1/blobs/{key}` | none | Local audio (unused when R2 is configured) |
| `GET` | `/v1/escrows` | none | Paginated list; `donor`, `recipient`, `state`, `expired` |
| `GET` | `/v1/escrows/{pubkey}` | none | One escrow, with its receipt if released |
| `POST` | `/v1/needs` | none, by design | Attach description text to an escrow |
| `GET` | `/v1/verify/{signature}` | none | Independent verification verdict |
| `GET` | `/v1/stats` | none | Aggregates and index lag |
| `GET` | `/healthz` | none | Liveness |

`POST /v1/needs` is unauthenticated on purpose and safe anyway: text is
accepted only if it hashes to the `need_hash` already committed on chain.
Anyone can call it; nobody can lie through it.

### Receipt pipeline ordering

1. Receive multipart audio; enforce size and duration caps.
2. Compute `audio_sha256`.
3. Write the blob, keyed by that hash (content-addressed, so retries are
   idempotent).
4. Transcribe. Failure here is not failure of the upload.
5. Normalise the transcript.
6. Build the `CanonicalReceipt`; compute `receipt_hash` via `cairn-core`.
7. Persist the hash → artifact mapping.
8. Return the hash and transcript for signing.

The blob is written **before** anything is signable. A transaction that later
fails leaves an orphaned object, which is harmless. The reverse order would
produce a committed on-chain hash pointing at bytes that were never stored,
which is not recoverable.

### Verification verdicts

| Verdict | Meaning |
|---|---|
| `verified` | Audio re-downloaded, re-hashed, and the receipt reproduces |
| `mismatch` | Recomputation does not reproduce the committed hash |
| `audio_unavailable` | Transcript and metadata reproduce, but the recording could not be fetched |
| `artifacts_unknown` | A hash is committed on chain; this server holds nothing for it |
| `transaction_failed` | The transaction itself failed, so it committed nothing |

A corrupted blob yields `mismatch`. A missing blob yields
`audio_unavailable`. Collapsing those two into one answer would be the single
most misleading thing this service could do.

## Edge cases

Where each case from the plan's register is actually handled.

| Case | Where |
|---|---|
| Release after the deadline | `submit_receipt` guard; UI disables and shows time remaining |
| Release twice | State guard; UI reflects the terminal state |
| Refund before the deadline | `refund` guard |
| Refund on a released escrow | State guard |
| `recipient == donor` | `create_escrow` guard, plus a client-side check |
| Amount below the minimum | `create_escrow` guard; the UI states the floor |
| Vault below rent exemption | Not reachable: the vault is dataless and always drained to zero |
| Audio upload fails mid-stream | No hash issued; no on-chain effect; client retries |
| Transcription fails | Hash computed over the audio with an empty transcript, marked unavailable |
| Transaction fails after upload | Blob orphaned; harmless |
| Declining the release prompt | `cairn receive` exits before building a transaction; nothing signed, nothing sent |
| Devnet RPC timeout | Signature surfaced immediately with an explorer link; confirmation polled with backoff |
| Devnet airdrop rate limited | The seeder refuses to airdrop and requires a pre-funded donor |
| Indexer poll fails | Logged, retried on the next tick; stale rows keep being served |
| Blob missing at verification | `audio_available: false`, distinct from `match: false` |
| Description altered off-chain | `cairn show` re-derives `need_hash` from the text it was served and prints TAMPERED |
| Zero-length audio | Rejected at upload (byte floor, plus a WAV header parse where the container allows one) |
| Audio over the cap | Rejected at upload; 10 MB / 120 s |

## Wire format

`crates/cairn-api/src/views.rs` holds every response shape as a Rust type with
both `Serialize` and `Deserialize`. The routes write them; `cairn-cli`
deserialises them. One definition per shape, which is the same argument as
`cairn-core` holding the hash and the account layout.

This was not possible while the client was a browser. A TypeScript frontend
means every response exists twice -- once as the `json!` the server emits and
once as the `type` the client declares -- and the two drift silently, because
nothing type-checks across the boundary. Removing the UI removed the second
copy of the wire format at the same time as it removed the second copy of the
hash.

## Versions, and why they are pinned where they are

The Solana crate split is mid-flight and the generations do not interoperate:

| Generation | `anchor-lang` | `solana-sdk` | `litesvm` | `solana-account` |
|---|---|---|---|---|
| older | 1.0 | 3.x | ≤0.13 | 3.x |
| current | 1.2 | 4.x | 0.16 | 4.x |

Mixing them gives you `Pubkey`, `Account` and `Transaction` types with
identical names, identical fields and no conversion between them, and error
messages that read like the compiler has lost its mind (`expected
solana_pubkey::Pubkey, found Address`). If a dependency bump produces that,
this table is the thing to check first.
