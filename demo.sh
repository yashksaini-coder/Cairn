#!/usr/bin/env bash
# Seeds a handful of escrows on devnet so there is something to look at.
#
# Deliberately mundane copy. A demo that leans on emotionally loaded text is
# testing the viewer's sympathy rather than the protocol.
set -euo pipefail

: "${CAIRN_PROGRAM_ID:?set CAIRN_PROGRAM_ID (see: anchor keys sync)}"
: "${DONOR_KEYPAIR:=$HOME/.config/solana/id.json}"
: "${RECIPIENT:?set RECIPIENT to a devnet pubkey that is not the donor}"
: "${CAIRN_API_URL:=http://localhost:8080}"
export CAIRN_API_URL

cairn=(cargo run --quiet -p cairn-cli --)

# No airdrop anywhere in here. Devnet rate-limits them, and a failed airdrop
# mid-demo looks exactly like a broken program.
echo "donor     $(solana address -k "$DONOR_KEYPAIR" 2>/dev/null || echo "$DONOR_KEYPAIR")"
echo "recipient $RECIPIENT"
echo

give() {
  "${cairn[@]}" give \
    --keypair "$DONOR_KEYPAIR" \
    --to "$RECIPIENT" \
    --amount "$1" \
    --window "$2" \
    --title "$3" \
    --for "$4"
  echo
}

give 0.02 24h "School fees, one term" \
  "Covers one term of school fees and a set of textbooks."

give 0.02 24h "Roof repair before the rains" \
  "Replaces four sheets of corrugated roofing on a single-room house."

# Deliberately short, so there is an escrow that expires during the demo and
# the refund path has something to act on.
give 0.02 5m  "Restocking a vegetable stall" \
  "Buys one week of stock for a vegetable stall after a closure."

echo "now:"
echo "  cairn list"
echo "  cairn receive --keypair <recipient> --escrow <pubkey> --audio receipt.wav"
