-- Read model. Every row here is derived from chain state or from artifacts
-- the server was handed; nothing in this database is authoritative, and
-- losing it entirely costs the project its cache, not its evidence.

CREATE TABLE IF NOT EXISTS escrows (
    pubkey        TEXT PRIMARY KEY,
    donor         TEXT    NOT NULL,
    recipient     TEXT    NOT NULL,
    amount        INTEGER NOT NULL,
    need_hash     TEXT    NOT NULL,   -- hex
    receipt_hash  TEXT,               -- hex, NULL until released
    deadline      INTEGER NOT NULL,
    state         TEXT    NOT NULL,   -- funded | released | refunded
    created_at    INTEGER NOT NULL,
    released_at   INTEGER,
    indexed_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS escrows_donor_idx     ON escrows(donor);
CREATE INDEX IF NOT EXISTS escrows_recipient_idx ON escrows(recipient);
-- §8.6 wanted a worker to flag expiries. This composite index makes
-- "funded and past deadline" a read-time query instead, which is both
-- simpler and never up to 60s stale.
CREATE INDEX IF NOT EXISTS escrows_state_idx     ON escrows(state, deadline);

-- The need text itself. Only the hash is on-chain; this table is what the
-- client re-hashes to detect tampering (§9.2).
CREATE TABLE IF NOT EXISTS needs (
    escrow      TEXT PRIMARY KEY,
    title       TEXT    NOT NULL,
    description TEXT    NOT NULL,   -- stored already normalised
    need_hash   TEXT    NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS receipts (
    receipt_hash         TEXT PRIMARY KEY,   -- hex
    escrow               TEXT    NOT NULL,
    audio_key            TEXT    NOT NULL,
    audio_sha256         TEXT    NOT NULL,   -- hex
    audio_bytes          INTEGER NOT NULL,
    content_type         TEXT    NOT NULL,
    transcript           TEXT    NOT NULL,   -- normalised; "" when unavailable
    transcript_available INTEGER NOT NULL,
    locale               TEXT    NOT NULL,
    recorded_at          INTEGER NOT NULL,
    created_at           INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS receipts_escrow_idx ON receipts(escrow);
