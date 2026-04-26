#!/usr/bin/env bash
#
# NOVAI AI-entity end-to-end demo.
#
# Walks the full lifecycle of an on-chain AI entity: keygen → faucet →
# register-with-key → credit → signal publish → memory CRUD → query, with
# banner sections suitable for a blog post or video.
#
# Requires a running local devnet (./scripts/devnet.sh). All state lives in
# /tmp/novai-demo-* so the script is repeatable; nothing is checked in.
#
# Usage:  bash demos/ai-entity-demo.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="$REPO_ROOT/target/release/novai-cli"
RPC="${NOVAI_RPC_URL:-http://localhost:3030}"

CREATOR_KEY=/tmp/novai-demo-creator.key
ENTITY_KEY=/tmp/novai-demo-entity.key

# ---------------------------------------------------------------------------
# Pretty-printing helpers
# ---------------------------------------------------------------------------

bar() {
  printf '\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n'
  printf ' %s\n' "$1"
  printf '━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n'
}

step() { printf '\n▸ %s\n' "$1"; }
note() { printf '  · %s\n' "$1"; }

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

if [[ ! -x "$CLI" ]]; then
  echo "novai-cli not built. Run: cargo build --release -p novai-cli" >&2
  exit 1
fi

if ! curl -s --max-time 2 -X POST "$RPC" \
     -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}' \
     >/dev/null; then
  echo "Cannot reach $RPC — is the devnet running? (./scripts/devnet.sh)" >&2
  exit 1
fi

CLI() { "$CLI" --endpoint "$RPC" "$@"; }
CLI_JSON() { "$CLI" --endpoint "$RPC" --json "$@"; }

# ---------------------------------------------------------------------------
# 1 — Generate creator + entity keypairs
# ---------------------------------------------------------------------------

bar '1 · Generate keypairs'

step 'Creator keypair (pays for registration):'
rm -f "$CREATOR_KEY"
CLI keygen --output "$CREATOR_KEY"
CREATOR=$(CLI key-info --key-file "$CREATOR_KEY" | awk '/^Address/ {print $2}')

step 'Entity keypair (the AI entity will sign with this):'
rm -f "$ENTITY_KEY"
CLI keygen --output "$ENTITY_KEY"
ENTITY_PK_ADDR=$(CLI key-info --key-file "$ENTITY_KEY" | awk '/^Address/ {print $2}')

# ---------------------------------------------------------------------------
# 2 — Fund creator + show balance
# ---------------------------------------------------------------------------

bar '2 · Fund the creator from the dev faucet'

CLI faucet --address "$CREATOR"
sleep 2
note 'Balance after faucet:'
CLI balance --address "$CREATOR"

# ---------------------------------------------------------------------------
# 3 — Register an AI entity
# ---------------------------------------------------------------------------

bar '3 · Register an AI entity with its own signing key'

CODE_HASH=0101010101010101010101010101010101010101010101010101010101010101
note 'code_hash is opaque on-chain — in production it would be the hash of'
note 'your AI agent code or model weights. Here it is a placeholder.'
echo

REG_OUT=$(CLI_JSON ai register-with-key \
  --key-file "$CREATOR_KEY" \
  --entity-key-file "$ENTITY_KEY" \
  --code-hash "$CODE_HASH" \
  --initial-balance 50000 \
  --fee 5000)
ENTITY_ID=$(echo "$REG_OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin)["entity_id"])')

step 'Registration submitted.'
note "Entity ID:      $ENTITY_ID"
note "Entity address: $ENTITY_PK_ADDR  (= blake3 of entity pubkey)"
sleep 2

step 'Entity record on chain:'
CLI ai info --entity-id "$ENTITY_ID"

# ---------------------------------------------------------------------------
# 4 — Credit the entity (top up its balance)
# ---------------------------------------------------------------------------

bar '4 · Credit the entity (top-up from creator)'

CLI ai credit \
  --key-file "$CREATOR_KEY" \
  --entity-id "$ENTITY_ID" \
  --amount 25000 \
  --fee 100
sleep 2

note 'New entity balance:'
CLI ai info --entity-id "$ENTITY_ID" | grep -E '^Balance' || true

# ---------------------------------------------------------------------------
# 5 — Publish a signal from the entity
# ---------------------------------------------------------------------------

bar '5 · Publish a signal (signed by the entity, not the creator)'

SIGNAL_HASH=0202020202020202020202020202020202020202020202020202020202020202
CLI signal publish \
  --key-file "$ENTITY_KEY" \
  --signal-hash "$SIGNAL_HASH" \
  --signal-type anomaly \
  --issuer-entity-id "$ENTITY_ID" \
  --fee 1000
sleep 2

step 'Signal indexed by issuer:'
LATEST_HEIGHT=$(curl -s -X POST "$RPC" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}' \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["height"])')
START=$(( LATEST_HEIGHT > 5000 ? LATEST_HEIGHT - 5000 : 0 ))
CLI signal by-issuer --issuer "$ENTITY_ID" --start "$START" --end "$LATEST_HEIGHT"

# ---------------------------------------------------------------------------
# 6 — Memory object CRUD
# ---------------------------------------------------------------------------

bar '6 · Memory object lifecycle (create → list → update → list)'

step 'Create:'
CREATE_OUT=$(CLI_JSON memory create \
  --key-file "$ENTITY_KEY" \
  --object-type chain-summary \
  --data 'demo summary v1: 10 blocks, 0 txs' \
  --fee 500)
echo "$CREATE_OUT" | python3 -m json.tool
sleep 2

step 'List (should show v1):'
CLI memory list --entity-id "$ENTITY_ID"

OBJECT_ID=$(curl -s -X POST "$RPC" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"novai_getMemoryObjects\",\"params\":{\"entity_id\":\"$ENTITY_ID\"},\"id\":1}" \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["objects"][0]["object_id"])')

step "Update object $OBJECT_ID:"
CLI memory update \
  --key-file "$ENTITY_KEY" \
  --object-id "$OBJECT_ID" \
  --data 'demo summary v2: 100 blocks, 12 txs' \
  --fee 500
sleep 2

step 'List again (should show v2):'
CLI memory list --entity-id "$ENTITY_ID"

# ---------------------------------------------------------------------------
# 7 — Final state summary
# ---------------------------------------------------------------------------

bar '7 · Final state summary'

note 'Creator account:'
CLI balance --address "$CREATOR"
echo

note 'Entity record:'
CLI ai info --entity-id "$ENTITY_ID"

bar '✓ Demo complete'
echo "  Entity ID:  $ENTITY_ID"
echo "  Entity addr: $ENTITY_PK_ADDR"
echo "  Creator:    $CREATOR"
echo
echo "  Look it up in the explorer: http://localhost:5173/entity/$ENTITY_ID"
echo "  (Run \`cd explorer && npm run dev\` if it isn't already.)"
echo
echo "  Keys are kept in /tmp/novai-demo-{creator,entity}.key for reuse;"
echo "  delete them and re-run to start fresh."
