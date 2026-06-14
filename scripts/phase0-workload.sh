#!/usr/bin/env bash
#
# phase0-workload.sh - drive the CLI-reachable handlers against the
# local 4-node devnet (RPC :3030) so the state_root sample taken by
# scripts/verify-local-devnet.sh has real, non-Transfer-heavy state
# to authenticate.
#
# Uses ONLY ./target/release/novai-cli flows. Does NOT hand-build
# governance payloads (no CLI driver yet, deferred by design). Does NOT
# exercise payment-request, service-attestation, sla, channel; those
# are out of scope for Phase 0 local coverage.
#
# Senders are the FIRST TEN dev-genesis accounts (apply_dev_genesis at
# crates/node/src/main.rs:568-618 seeds 100 accounts at 1e9 each). The
# seed-derivation algorithm is replayed in write_genesis_seed below.
# No faucet, no funding transfers, no rate-limit risk.
#
# Role assignment (avoids the type-8 CreatorAlreadyHasEntity rule at
# crates/execution/src/lib.rs:9124-9132, and avoids per-sender nonce
# contention across sections):
#   sender-0:    ai credit creditor (3 credits to sender-1/2/3 entities)
#   sender-1:    type-8 ORACLE entity creator AND oracle post-anchor signer
#   sender-2:    type-8 standard entity creator
#   sender-3:    type-8 standard entity creator
#   sender-4:    type-10 register-with-key creator (3 entities with own keys)
#   sender-5..9: transfer control only (only the sender's nonce advances
#                on a transfer; the recipient's nonce is preserved per
#                crates/execution/src/lib.rs:6659-6748)
#
# Classes counted:
#   transfer                 50 transfers (10 rounds across senders 5..9)
#   ai_register              3 type-8 registrations
#   ai_register_with_key     3 type-10 registrations
#   ai_credit                3 credits, one per type-8 entity
#   oracle_post_anchor       5 anchors signed by sender-1.key
#   memory_create            3 creations on type-10 #1 (signed by entity-1.key)
#   memory_update            up to 3 updates of those objects
#   memory_delete            up to 3 deletes of those objects
#
# JSON field names confirmed against source:
#   ai info --json (exists path): top-level .id (hex), .nonce (u64),
#     per AiEntityJson at crates/node/src/rpc.rs:978-1001 plus the
#     unwrap of .entity at tools/novai-cli/src/rpc_client.rs:151-156.
#   ai info --json (not-found path): literally {"entity": null}, so
#     has("id") is false until commit.
#   balance --json: {balance:u64, nonce:u64} per
#     tools/novai-cli/src/commands/account.rs:46-50.
#   nonce --json: {nonce:u64} per same file:60-66.
#
# Commit-waiting: every section polls committed state.
#   - ai_register / ai_register_with_key: poll ai info until has("id").
#   - ai_credit / oracle_post_anchor: poll the signer account's nonce.
#   - memory_create: poll memory list set difference (object_id is
#     server-derived; not returned by memory create --json).
#   - memory_update / memory_delete: poll the entity's nonce (signer
#     is the type-10 entity's own key).
#
# Output: per-class accept vs reject count to stdout at end.
# Raw CLI output is appended to /tmp/phase0-workload.log for diagnosis.
#
# Bash 3.2 compatible. No associative arrays. Uses eval-by-name for
# per-class counters.

set -uo pipefail

CLI="${CLI:-./target/release/novai-cli}"
ENDPOINT="${ENDPOINT:-http://localhost:3030}"
KEYDIR="${KEYDIR:-/tmp/phase0-keys}"
LOG="${LOG:-/tmp/phase0-workload.log}"

mkdir -p "$KEYDIR"
: > "$LOG"

CLASSES="transfer ai_register ai_register_with_key ai_credit oracle_post_anchor memory_create memory_update memory_delete"
for cls in $CLASSES; do
  eval "ACCEPT_${cls}=0"
  eval "REJECT_${cls}=0"
done

# ─── helpers ──────────────────────────────────────────────────────────────────

extract_address() {
  awk '/^Address:/ {print $2; exit}' "$1"
}

# Write the 32-byte ed25519 seed for genesis dev account index N (0..255)
# matching apply_dev_genesis in crates/node/src/main.rs:579-587:
#   seed_byte = (index % 256) as u8
#   seed = [seed_byte; 32]; index_bytes = (index as u64).to_le_bytes()
#   for j in 0..8: seed[j] ^= index_bytes[j]
# For index in [0, 255], index_bytes[0] = index and index_bytes[1..8] = 0,
# so seed[0] = index ^ index = 0 and seed[1..32] = index unchanged.
write_genesis_seed() {
  local idx="$1" path="$2"
  if (( idx < 0 || idx > 255 )); then
    echo "FATAL: write_genesis_seed only supports 0..255, got $idx" >&2
    return 2
  fi
  local hex_idx
  hex_idx="$(printf '%02x' "$idx")"
  local hex_string="00"
  local i
  for ((i = 1; i < 32; i++)); do
    hex_string="${hex_string}${hex_idx}"
  done
  printf '%s' "$hex_string" | xxd -r -p > "$path"
  chmod 600 "$path"
}

balance_of() {
  local addr="$1"
  "$CLI" --endpoint "$ENDPOINT" --json balance --address "$addr" 2>/dev/null \
    | jq -r '.balance // 0' 2>/dev/null
}

account_nonce_of() {
  local addr="$1"
  "$CLI" --endpoint "$ENDPOINT" --json nonce --address "$addr" 2>/dev/null \
    | jq -r '.nonce // 0' 2>/dev/null
}

entity_exists() {
  local eid="$1"
  "$CLI" --endpoint "$ENDPOINT" --json ai info --entity-id "$eid" 2>/dev/null \
    | jq -e 'has("id")' >/dev/null 2>&1
}

entity_nonce_of() {
  local eid="$1"
  "$CLI" --endpoint "$ENDPOINT" --json ai info --entity-id "$eid" 2>/dev/null \
    | jq -r '.nonce // 0' 2>/dev/null
}

get_height() {
  curl -s --max-time 3 \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":[],"id":1}' \
    "$ENDPOINT" 2>/dev/null \
    | jq -r '.result.height // 0' 2>/dev/null
}

poll_until_nonce_at_least() {
  local addr="$1" target="$2" timeout="${3:-15}" start_ts cur
  start_ts=$(date +%s)
  while :; do
    cur="$(account_nonce_of "$addr")"
    cur="${cur:-0}"
    if (( cur >= target )); then
      return 0
    fi
    if (( $(date +%s) - start_ts >= timeout )); then
      return 1
    fi
    sleep 1
  done
}

poll_until_entity_exists() {
  local eid="$1" timeout="${2:-15}" start_ts
  start_ts=$(date +%s)
  while :; do
    if entity_exists "$eid"; then
      return 0
    fi
    if (( $(date +%s) - start_ts >= timeout )); then
      return 1
    fi
    sleep 1
  done
}

poll_until_entity_nonce_at_least() {
  local eid="$1" target="$2" timeout="${3:-15}" start_ts cur
  start_ts=$(date +%s)
  while :; do
    cur="$(entity_nonce_of "$eid")"
    cur="${cur:-0}"
    if (( cur >= target )); then
      return 0
    fi
    if (( $(date +%s) - start_ts >= timeout )); then
      return 1
    fi
    sleep 1
  done
}

snapshot_object_ids() {
  local entity_id="$1" out
  out="$("$CLI" --endpoint "$ENDPOINT" --json memory list \
    --entity-id "$entity_id" 2>/dev/null \
    | jq -c '[.objects[].object_id]' 2>/dev/null)"
  if [[ -z "$out" || "$out" == "null" ]]; then
    echo '[]'
  else
    echo "$out"
  fi
}

poll_until_new_object() {
  local entity_id="$1" pre_ids="$2" timeout="${3:-15}"
  local start_ts post_ids new_id
  start_ts=$(date +%s)
  while :; do
    post_ids="$(snapshot_object_ids "$entity_id")"
    new_id="$(jq -nr \
      --argjson pre "$pre_ids" \
      --argjson post "$post_ids" \
      '($post - $pre)[0] // empty' 2>/dev/null)"
    if [[ -n "$new_id" ]]; then
      echo "$new_id"
      return 0
    fi
    if (( $(date +%s) - start_ts >= timeout )); then
      return 1
    fi
    sleep 1
  done
}

# run_count: count a tx by class, with commit-accept semantics. CLI exit 0
# is only mempool-accept; handlers can still reject at execution time
# (duplicate registrations, duplicate signal hashes on a re-run with
# identical inputs). So ACCEPT requires CLI exit 0 AND post-tx nonce
# advance for the sender. CLI exit 0 without commit observation inside
# the timeout is REJECT plus a log line tagged "mempool accept, no commit".
# Args: cls sender_addr -- cmd ...
run_count() {
  local cls="$1" sender_addr="$2"; shift 2
  local pre_n
  pre_n="$(account_nonce_of "$sender_addr")"
  pre_n="${pre_n:-0}"
  if "$@" >>"$LOG" 2>&1; then
    if poll_until_nonce_at_least "$sender_addr" "$((pre_n + 1))" 15; then
      eval "ACCEPT_${cls}=\$((ACCEPT_${cls} + 1))"
    else
      eval "REJECT_${cls}=\$((REJECT_${cls} + 1))"
      echo "run_count($cls): mempool accept, no commit observed in 15s (pre_nonce=${pre_n}, sender=${sender_addr})" >>"$LOG"
    fi
  else
    eval "REJECT_${cls}=\$((REJECT_${cls} + 1))"
  fi
}

submit_and_advance_account() {
  local cls="$1" sender_addr="$2"; shift 2
  local pre_n
  pre_n="$(account_nonce_of "$sender_addr")"
  pre_n="${pre_n:-0}"
  if "$@" >>"$LOG" 2>&1; then
    eval "ACCEPT_${cls}=\$((ACCEPT_${cls} + 1))"
    poll_until_nonce_at_least "$sender_addr" "$((pre_n + 1))" 15 \
      || echo "submit_and_advance_account($cls): nonce did not advance from $pre_n in 15s" >>"$LOG"
  else
    eval "REJECT_${cls}=\$((REJECT_${cls} + 1))"
  fi
}

submit_and_advance_entity() {
  local cls="$1" entity_id="$2"; shift 2
  local pre_n
  pre_n="$(entity_nonce_of "$entity_id")"
  pre_n="${pre_n:-0}"
  if "$@" >>"$LOG" 2>&1; then
    eval "ACCEPT_${cls}=\$((ACCEPT_${cls} + 1))"
    poll_until_entity_nonce_at_least "$entity_id" "$((pre_n + 1))" 15 \
      || echo "submit_and_advance_entity($cls): entity nonce did not advance from $pre_n in 15s" >>"$LOG"
  else
    eval "REJECT_${cls}=\$((REJECT_${cls} + 1))"
  fi
}

# ─── Preflight ────────────────────────────────────────────────────────────────

for tool in curl jq awk xxd; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FATAL: required tool not found: $tool" >&2
    exit 2
  fi
done
if [[ ! -x "$CLI" ]]; then
  echo "FATAL: $CLI not found or not executable" >&2; exit 2
fi
start_h="$(get_height)"
if [[ -z "$start_h" || "$start_h" == "null" ]]; then
  echo "FATAL: RPC $ENDPOINT did not return a height. Is devnet running?" >&2
  exit 2
fi
echo "Starting workload at height ${start_h}, endpoint ${ENDPOINT}"
echo "Keys: ${KEYDIR}    Log: ${LOG}"

# ─── 1. Genesis senders + balance verification ────────────────────────────────

SENDERS=()
for i in 0 1 2 3 4 5 6 7 8 9; do
  kf="${KEYDIR}/sender-${i}.key"
  write_genesis_seed "$i" "$kf"
  "$CLI" key-info --key-file "$kf" >"${KEYDIR}/sender-${i}.out" 2>&1 \
    || { echo "FATAL: key-info sender-${i} failed" >&2; cat "${KEYDIR}/sender-${i}.out" >&2; exit 2; }
  addr="$(extract_address "${KEYDIR}/sender-${i}.out")"
  if [[ -z "$addr" ]]; then
    echo "FATAL: could not extract sender-${i} address from key-info output" >&2
    exit 2
  fi
  bal="$(balance_of "$addr")"
  if [[ -z "$bal" || "$bal" == "0" || "$bal" == "null" ]]; then
    echo "FATAL: sender-${i} address $addr has zero balance." >&2
    echo "       Expected dev-genesis funding from apply_dev_genesis at" >&2
    echo "       crates/node/src/main.rs:568-618. Aborting before any tx." >&2
    exit 3
  fi
  SENDERS[${#SENDERS[@]}]="$addr"
  echo "sender-${i} = ${addr} (balance ${bal})" >>"$LOG"
done
echo "Senders ready: ${#SENDERS[@]} (all genesis-funded, balance verified)"

# ─── 2. Transfer control (50 txs, 10 rounds * senders 5..9) ───────────────────

# Capture each sender's nonce at run start so the per-round poll target is
# absolute (pre_nonce + round), not relative to zero. On a re-run after a
# prior pass, senders' nonces are already at least 10. A $round-relative
# target returned immediately and back-to-back rounds re-submitted
# identical (sender, recipient, amount, fee, nonce) tuples that the
# mempool deduped by TxID, masking the work as completed.
PRE_NONCE_TRANSFER=()
for i in 5 6 7 8 9; do
  pn="$(account_nonce_of "${SENDERS[$i]}")"
  PRE_NONCE_TRANSFER[$i]="${pn:-0}"
done

for round in 1 2 3 4 5 6 7 8 9 10; do
  for i in 5 6 7 8 9; do
    from_key="${KEYDIR}/sender-${i}.key"
    to_idx=$(( i + 1 ))
    if (( to_idx > 9 )); then to_idx=5; fi
    to_addr="${SENDERS[$to_idx]}"
    run_count transfer "${SENDERS[$i]}" "$CLI" --endpoint "$ENDPOINT" transfer \
      --key-file "$from_key" \
      --to "$to_addr" \
      --amount 100 \
      --fee 100
  done
  target=$(( PRE_NONCE_TRANSFER[5] + round ))
  poll_until_nonce_at_least "${SENDERS[5]}" "$target" 10 \
    || echo "transfer round $round: sender-5 nonce did not advance to $target in 10s" >>"$LOG"
done

# ─── 3. ai register (type 8): #1 oracle, #2/#3 standard ───────────────────────

REG_ENTITY_IDS=()
ORACLE_EID=""

out1="$("$CLI" --endpoint "$ENDPOINT" --json ai register \
  --key-file "${KEYDIR}/sender-1.key" \
  --code-hash "$(printf 'a%063d' 1)" \
  --autonomy advisory \
  --capabilities "read_chain,read_memory,emit_proposals,post_oracle_anchors" \
  --initial-balance 10000000 \
  --fee 5000 2>&1)"
echo "ai register #1 oracle (sender-1 creator): ${out1}" >>"$LOG"
if echo "$out1" | jq -e '.entity_id' >/dev/null 2>&1; then
  cand="$(echo "$out1" | jq -r '.entity_id')"
  if poll_until_entity_exists "$cand" 15; then
    ORACLE_EID="$cand"
    REG_ENTITY_IDS[${#REG_ENTITY_IDS[@]}]="$ORACLE_EID"
    ACCEPT_ai_register=$((ACCEPT_ai_register + 1))
  else
    REJECT_ai_register=$((REJECT_ai_register + 1))
    echo "ai register #1: entity_id $cand did not appear in 15s" >>"$LOG"
  fi
else
  REJECT_ai_register=$((REJECT_ai_register + 1))
fi

for n in 2 3; do
  out="$("$CLI" --endpoint "$ENDPOINT" --json ai register \
    --key-file "${KEYDIR}/sender-${n}.key" \
    --code-hash "$(printf 'a%063d' "$n")" \
    --autonomy advisory \
    --capabilities "read_chain,read_memory,emit_proposals" \
    --initial-balance 10000000 \
    --fee 5000 2>&1)"
  echo "ai register #${n} standard (sender-${n} creator): ${out}" >>"$LOG"
  if echo "$out" | jq -e '.entity_id' >/dev/null 2>&1; then
    eid="$(echo "$out" | jq -r '.entity_id')"
    if poll_until_entity_exists "$eid" 15; then
      REG_ENTITY_IDS[${#REG_ENTITY_IDS[@]}]="$eid"
      ACCEPT_ai_register=$((ACCEPT_ai_register + 1))
    else
      REJECT_ai_register=$((REJECT_ai_register + 1))
      echo "ai register #${n}: entity_id $eid did not appear in 15s" >>"$LOG"
    fi
  else
    REJECT_ai_register=$((REJECT_ai_register + 1))
  fi
done

# ─── 4. ai register-with-key (type 10) via sender-4 ───────────────────────────

REG_KEY_ENTITY_IDS=()
for n in 1 2 3; do
  ek="${KEYDIR}/entity-${n}.key"
  if [[ ! -f "$ek" ]]; then
    "$CLI" keygen --output "$ek" >/dev/null 2>&1 \
      || echo "keygen entity-${n} failed" >>"$LOG"
  fi
  out="$("$CLI" --endpoint "$ENDPOINT" --json ai register-with-key \
    --key-file "${KEYDIR}/sender-4.key" \
    --entity-key-file "$ek" \
    --code-hash "$(printf 'b%063d' "$n")" \
    --autonomy advisory \
    --capabilities "read_chain,read_memory,emit_proposals" \
    --initial-balance 10000000 \
    --fee 5000 2>&1)"
  echo "ai register-with-key #${n} (sender-4 creator): ${out}" >>"$LOG"
  if echo "$out" | jq -e '.entity_id' >/dev/null 2>&1; then
    eid="$(echo "$out" | jq -r '.entity_id')"
    if poll_until_entity_exists "$eid" 15; then
      REG_KEY_ENTITY_IDS[${#REG_KEY_ENTITY_IDS[@]}]="$eid"
      ACCEPT_ai_register_with_key=$((ACCEPT_ai_register_with_key + 1))
    else
      REJECT_ai_register_with_key=$((REJECT_ai_register_with_key + 1))
      echo "ai register-with-key #${n}: entity_id $eid did not appear in 15s" >>"$LOG"
    fi
  else
    REJECT_ai_register_with_key=$((REJECT_ai_register_with_key + 1))
  fi
done

# ─── 5. ai credit (sender-0 credits each type-8 entity) ───────────────────────

for eid in "${REG_ENTITY_IDS[@]}"; do
  submit_and_advance_account ai_credit "${SENDERS[0]}" \
    "$CLI" --endpoint "$ENDPOINT" ai credit \
    --key-file "${KEYDIR}/sender-0.key" \
    --entity-id "$eid" \
    --amount 1000000 \
    --fee 100
done

# ─── 6. oracle post-anchor (sender-1.key signs ORACLE_EID) ────────────────────
# Signer-to-issuer resolution: the on-chain dispatch resolves the
# issuer via lookup_ai_entity_by_address(&tx.from) at
# crates/execution/src/lib.rs:7235. For type-8 entities, the reverse
# index is keyed on the creator address per
# crates/execution/src/lib.rs:9124-9132. sender-1 is the creator of
# ORACLE_EID, so signing with sender-1.key gives tx.from = sender-1.addr
# which resolves correctly. This is the same path that wedged the
# production price-oracle at IssuerNotFound; avoided here by matching the
# signer to the type-8 creator and ensuring sender-1 has registered
# exactly one type-8 entity (so the reverse index points at ORACLE_EID).

if [[ -n "$ORACLE_EID" ]]; then
  for n in 1 2 3 4 5; do
    data_hash="$(printf 'c%063d' "$n")"
    ts=$((1700000000 + n))
    tag="price/ETH-USD-${n}"
    submit_and_advance_account oracle_post_anchor "${SENDERS[1]}" \
      "$CLI" --endpoint "$ENDPOINT" oracle post-anchor \
      --key-file "${KEYDIR}/sender-1.key" \
      --issuer-entity-id "$ORACLE_EID" \
      --data-hash "$data_hash" \
      --external-timestamp "$ts" \
      --data-tag "$tag" \
      --fee 1000
  done
else
  echo "oracle_post_anchor SKIPPED: ORACLE_EID not available" >>"$LOG"
fi

# ─── 7. memory create / update / delete chains on type-10 entity #1 ───────────
# Object-id resolution uses pre/post set difference because
# novai_getMemoryObjects returns objects in lex order of the object_id
# hash (crates/execution/src/lib.rs:11164-11184), not creation order,
# and memory create --json does not surface the object_id
# (tools/novai-cli/src/commands/memory.rs:65-73). Memory update and
# delete are signed by the entity's OWN key, so commit-waiting polls
# the entity's nonce via novai-cli ai info, not the creator account.

if (( ${#REG_KEY_ENTITY_IDS[@]} > 0 )); then
  MEM_EID="${REG_KEY_ENTITY_IDS[0]}"
  MEM_KEY="${KEYDIR}/entity-1.key"
  for n in 1 2 3; do
    pre_ids="$(snapshot_object_ids "$MEM_EID")"

    if "$CLI" --endpoint "$ENDPOINT" memory create \
      --key-file "$MEM_KEY" \
      --object-type chain-summary \
      --data "phase0-mem-${n}" \
      --fee 500 >>"$LOG" 2>&1
    then
      ACCEPT_memory_create=$((ACCEPT_memory_create + 1))
    else
      REJECT_memory_create=$((REJECT_memory_create + 1))
      echo "memory chain #${n}: create REJECTED, skipping update/delete" >>"$LOG"
      continue
    fi

    obj_id="$(poll_until_new_object "$MEM_EID" "$pre_ids" 15)"
    if [[ -z "$obj_id" ]]; then
      echo "memory chain #${n}: timed out waiting for new object_id" >>"$LOG"
      continue
    fi
    echo "memory chain #${n}: new object_id=${obj_id}" >>"$LOG"

    submit_and_advance_entity memory_update "$MEM_EID" \
      "$CLI" --endpoint "$ENDPOINT" memory update \
      --key-file "$MEM_KEY" \
      --object-id "$obj_id" \
      --data "phase0-mem-${n}-updated" \
      --fee 500

    submit_and_advance_entity memory_delete "$MEM_EID" \
      "$CLI" --endpoint "$ENDPOINT" memory delete \
      --key-file "$MEM_KEY" \
      --object-id "$obj_id" \
      --fee 500
  done
else
  echo "memory chains SKIPPED: no type-10 entity available" >>"$LOG"
fi

# ─── 8. Final tally ───────────────────────────────────────────────────────────

end_h="$(get_height)"

echo
echo "=== PHASE 0 WORKLOAD RESULTS ==="
echo "  endpoint:             ${ENDPOINT}"
echo "  start height:         ${start_h}"
echo "  end height:           ${end_h}"
echo "  raw log:              ${LOG}"
echo
printf '  %-26s %8s %8s\n' "class" "accept" "reject"
printf '  %-26s %8s %8s\n' "-----" "------" "------"
for cls in $CLASSES; do
  a="$(eval "echo \$ACCEPT_${cls}")"
  r="$(eval "echo \$REJECT_${cls}")"
  printf '  %-26s %8s %8s\n' "$cls" "$a" "$r"
done
echo
echo "Notes:"
echo "  - 'accept' here means CLI exit 0 AND the post-tx committed state"
echo "    was observed (nonce or entity or object). It is a closer proxy"
echo "    for committed-to-state than mempool-accepted."
echo "  - oracle_post_anchor success is the pass condition for that"
echo "    class: each accepted anchor exercises apply_state_ops_with_smt"
echo "    via the OracleAnchor branch at"
echo "    crates/execution/src/lib.rs:8980-8985."
echo
echo "Run scripts/verify-local-devnet.sh next."
