#!/usr/bin/env bash
# Records the Cairn demo against a live local validator.
#
# Everything here is executed for real: the escrow is created on chain, the
# receipt is signed by the recipient's key, the verifier re-downloads and
# re-hashes the recording, and the failure at the end is a genuine hash
# mismatch. Nothing is staged, and no output is replayed.
#
# Needs `just validator` and `just api` already running. Run it through
# `just record-demo`, which wraps it in asciinema.
set -uo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
export CLUSTER=local CLICOLOR_FORCE=1

OFF=$'\033[0m'; ACCENT=$'\033[33m'
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# Typed a character at a time. A demo where whole commands appear at once
# reads as a slideshow of screenshots rather than a session.
type_out() {
  local s=$1 i
  for (( i = 0; i < ${#s}; i++ )); do
    printf '%s' "${s:i:1}"
    sleep 0.018
  done
}

# Shows the command being typed, runs it for real, keeps a copy of the output
# so later steps can read the escrow and signature out of it rather than
# guessing at them.
run() {
  local cmd=$1 pause=${2:-2.2}
  printf '%s❯%s ' "$ACCENT" "$OFF"
  type_out "$cmd"
  sleep 0.35
  printf '\n'
  # just prints its own "recipe failed on line N" after any non-zero recipe.
  # That is task-runner noise citing a line number that drifts; the program's
  # own error message is left alone.
  eval "$cmd" 2>&1 | grep -vE '^error: recipe .* failed on line' | tee "$TMP/last"
  sleep "$pause"
}

field() { sed -r 's/\x1b\[[0-9;]*m//g' "$TMP/last" | awk -v k="$1" '$1 == k { print $2 }' | head -1; }

RECIPIENT=$(solana address -k .demo/recipient.json)

clear; sleep 0.25

run "just give $RECIPIENT 0.05 \"Covers one term of school fees and a set of textbooks.\"" 2.6
ESCROW=$(field escrow)

run "just show $ESCROW" 3.2

clear; sleep 0.6

run "just sample-audio receipt.wav" 1.4
run "just receive $ESCROW receipt.wav" 3.0
SIGNATURE=$(sed -r 's/\x1b\[[0-9;]*m//g' "$TMP/last" | grep -oE 'tx/[A-Za-z0-9]+' | head -1 | cut -d/ -f2)

clear; sleep 0.6

run "just verify $SIGNATURE" 3.4
run "just demo-tamper $ESCROW" 1.8
run "just verify $SIGNATURE" 4.5

printf '\n'
sleep 1.5
