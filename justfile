# Cairn task runner.  https://github.com/casey/just
#
#   cargo install just     # once
#   just                   # list everything
#
# Chosen over a Makefile because half of these recipes take arguments
# (`just give <pubkey> 0.05 "..."`), which make only does through
# $(MAKECMDGOALS) hacks. Nothing here is a build rule; nothing needs
# timestamp dependency tracking.

set dotenv-load := true
set positional-arguments := true

# `local` for a test validator, `devnet` for the real thing.
cluster    := env_var_or_default("CLUSTER", "local")
rpc        := if cluster == "local" { "http://127.0.0.1:8899" } else { "https://api.devnet.solana.com" }
api_url    := env_var_or_default("CAIRN_API_URL", "http://localhost:8080")

# Self-contained so nothing ever touches ~/.config/solana/id.json.
keys       := justfile_directory() / ".demo"
program_kp := keys / "program.json"
donor      := keys / "donor.json"
recipient  := keys / "recipient.json"
solana_bin := env_var("HOME") / ".local/share/solana/install/active_release/bin"

export PATH := solana_bin + ":" + env_var("PATH")

_default:
    @just --list --unsorted

# ---------------------------------------------------------------- setup

# Install the Agave toolchain (Anchor CLI is not required -- see docs/COMMANDS.md).
install-toolchain:
    sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"

# Generate the program, donor and recipient keypairs into .demo/ (gitignored).
keys:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "{{ keys }}"
    for k in program donor recipient; do
      if [ ! -f "{{ keys }}/$k.json" ]; then
        solana-keygen new --no-bip39-passphrase -s -o "{{ keys }}/$k.json" >/dev/null
        echo "created $k"
      fi
    done
    printf '%-10s %s\n' program "$(solana address -k {{ program_kp }})" \
                        donor   "$(solana address -k {{ donor }})" \
                        recipient "$(solana address -k {{ recipient }})"

# This is `anchor keys sync`, done without needing Anchor installed.

# Write the program keypair's pubkey into lib.rs and Anchor.toml.
sync-id: keys
    #!/usr/bin/env bash
    set -euo pipefail
    id="$(solana address -k {{ program_kp }})"
    sed -i "s|declare_id!(\"[^\"]*\")|declare_id!(\"$id\")|" programs/cairn/src/lib.rs
    sed -i "s|^cairn = \"[^\"]*\"$|cairn = \"$id\"|" Anchor.toml
    echo "program id: $id"

# ---------------------------------------------------------------- build & check

build:
    cargo build --workspace

# `--arch v3` is not optional. cargo-build-sbf still defaults to v0, and
# SIMD-0500 disables deployment of v0, v1 and v2 on every current cluster --
# so the default build produces a .so that nothing will accept, and says so
# only at deploy time as "sbpf_version ... not enabled".

# Compile the program to SBF. Emits target/deploy/cairn.so.
build-program:
    cargo build-sbf --manifest-path programs/cairn/Cargo.toml --arch v3

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Unit tests. The LiteSVM suite needs a built .so -- see test-program.
test:
    cargo test --workspace --exclude cairn

# Every state transition and every guard, in-process.
test-program: build-program
    cargo test -p cairn --test litesvm

# What CI runs. Run this before you push.
check: fmt lint test test-program
    @echo "all clear"

# ---------------------------------------------------------------- chain

# Start a local validator. Leave it running in its own terminal.
validator:
    solana-test-validator --reset --quiet --ledger {{ keys }}/ledger

# Localnet only: devnet airdrops are rate-limited, and Cairn itself never
# airdrops on your behalf -- a failed airdrop mid-demo looks exactly like a
# broken program.

# Put SOL in the donor and recipient wallets.
fund amount="10":
    #!/usr/bin/env bash
    set -euo pipefail
    for who in {{ donor }} {{ recipient }}; do
      solana airdrop {{ amount }} "$(solana address -k $who)" --url {{ rpc }} >/dev/null
    done
    printf '%-10s %s\n' donor     "$(solana balance -k {{ donor }} --url {{ rpc }})" \
                        recipient "$(solana balance -k {{ recipient }} --url {{ rpc }})"

# The program keypair fixes the address, so redeploying upgrades in place
# rather than landing at a new one.

# Build and deploy the program.
deploy: build-program
    #!/usr/bin/env bash
    set -euo pipefail
    solana program deploy target/deploy/cairn.so \
      --program-id {{ program_kp }} \
      --keypair {{ donor }} \
      --url {{ rpc }}
    echo "deployed: $(solana address -k {{ program_kp }}) on {{ cluster }}"

# ---------------------------------------------------------------- run

# It serves the index, stores blobs and recomputes hashes. It holds no keys
# and has no instruction it could call to move a lamport.

# Start the API.
api:
    #!/usr/bin/env bash
    set -euo pipefail
    export CAIRN_PROGRAM_ID="$(solana address -k {{ program_kp }})"
    export SOLANA_RPC_URL="{{ rpc }}"
    cargo run -p cairn-api

# ---------------------------------------------------------------- client

_cli := "cargo run --quiet -p cairn-cli --"

# Everything the CLI needs to reach the right chain and server.
_env := "CAIRN_PROGRAM_ID=$(solana address -k " + program_kp + ") SOLANA_RPC_URL=" + rpc + " CAIRN_API_URL=" + api_url

# Lock funds for someone.  just give <pubkey> 0.05 "one term of school fees"
give to amount description window="24h":
    @env {{ _env }} {{ _cli }} give --keypair {{ donor }} --to "{{ to }}" \
      --amount "{{ amount }}" --for "{{ description }}" --window "{{ window }}"

# Record a receipt and release the funds.  just receive <escrow> receipt.wav
receive escrow audio="receipt.wav":
    @env {{ _env }} {{ _cli }} receive --keypair {{ recipient }} \
      --escrow "{{ escrow }}" --audio "{{ audio }}" --yes

# Take back an expired escrow.  just refund <escrow>
refund escrow:
    @env {{ _env }} {{ _cli }} refund --keypair {{ donor }} --escrow "{{ escrow }}"

list *args:
    @env {{ _env }} {{ _cli }} list "$@"

# One escrow, cross-checked against the chain.
show escrow:
    @env {{ _env }} {{ _cli }} show "{{ escrow }}"

# Re-fetch, re-hash, recompute, compare.
verify signature:
    @env {{ _env }} {{ _cli }} verify "{{ signature }}"

# Recompute a hash offline. No server, no key, no network.
# `"$@"`, not `{{ args }}`: the latter word-splits, so a quoted description
# arrives as a dozen separate arguments and clap rejects the second one.
hash *args:
    @env {{ _env }} {{ _cli }} hash "$@"

# ---------------------------------------------------------------- demo

# Real use is a microphone: arecord -f cd -d 10 receipt.wav

# Generate a placeholder WAV to stand in for a recording.
sample-audio out="receipt.wav" seconds="6":
    @ffmpeg -y -loglevel error -f lavfi -i "sine=frequency=220:duration={{ seconds }}" \
      -ac 1 -ar 16000 -c:a pcm_s16le "{{ out }}"
    @echo "wrote {{ out }} ({{ seconds }}s)"

# Seed a few escrows so there is something to look at.
seed: keys
    @DONOR_KEYPAIR={{ donor }} RECIPIENT="$(solana address -k {{ recipient }})" \
      CAIRN_PROGRAM_ID="$(solana address -k {{ program_kp }})" \
      SOLANA_RPC_URL={{ rpc }} CAIRN_API_URL={{ api_url }} ./demo.sh

# ---------------------------------------------------------------- housekeeping

clean:
    cargo clean
    rm -f cairn.db cairn.db-shm cairn.db-wal
    rm -rf blobs

# Delete the local chain state and start over. Keeps the keypairs.
reset-chain:
    rm -rf {{ keys }}/ledger cairn.db cairn.db-shm cairn.db-wal blobs
