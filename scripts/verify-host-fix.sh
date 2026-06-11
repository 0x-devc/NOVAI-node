#!/usr/bin/env bash
#
# verify-host-fix.sh - post-deploy verification for the SMT inclusion fix.
#
# Runs on the [redacted-host] host (root@[redacted-ip]). Staged at /tmp/ per Section H.0 of
# docs/deploy-bug1-fix.md. Called many times across the post-deploy timeline
# (T+1m, T+5m, T+10m, T+20m, T+1h..T+24h per Section H.1). Designed to be
# under 10 seconds wall-clock and safe to re-run.
#
# Collects 8 categories of evidence in a single local run, formatted for
# direct paste into claude.ai for review:
#   1. Systemd unit states for all 6 services (4 validators + monitor + oracle)
#   2. Latest committed block per validator (4 ports)
#   3. Per-validator state_root at 4 heights (head, head-10, head-100, head-1000)
#   4. AGREE / DIVERGE verdict per height with dissenting port highlighted
#   5. Oracle entity record (nonce, capabilities, is_active) - when address supplied
#   6. Disk usage on / and /var/lib/novai
#   7. Last 20 journal lines for novai-node@0
#   8. Grep for "State root mismatch" across all 4 validator journals in the last hour
#
# Exit code:
#   0 - all 4 validators AGREE at all 4 heights AND no State root mismatch lines.
#   1 - any DIVERGE, any down validator, OR any State root mismatch line found.
#   2 - environment failure (missing curl/jq, RPC unreachable on @0).
#
# Input:
#   ORACLE_ADDR_HEX (env var OR first positional arg) - the address recorded in
#   Section G.2 of docs/deploy-bug1-fix.md as `<ORACLE_ADDR_HEX>`. Optional;
#   defaults to empty, which skips evidence section 5 with a NOT PROVIDED notice.
#   When supplied, the script reads entity_id_hex from /etc/novai/oracle-keys.json
#   (the keyfile that bootstrap.py writes per agents/price-oracle/bootstrap.py:86-99)
#   and queries novai_getAiEntity with that entity_id. The address itself is used
#   only as a cross-check against the keyfile.

set -euo pipefail

# ─── Constants ───────────────────────────────────────────────────────────────

readonly VALIDATOR_PORTS=(3030 3031 3032 3033)
readonly VALIDATOR_INDICES=(0 1 2 3)
readonly SERVICES=(
  "novai-node@0"
  "novai-node@1"
  "novai-node@2"
  "novai-node@3"
  "novai-monitor"
  "novai-price-oracle"
)
readonly ORACLE_KEYFILE="/etc/novai/oracle-keys.json"
readonly CURL_TIMEOUT=3
readonly HEAD_OFFSETS=(0 10 100 1000)  # head, head-10, head-100, head-1000

# ─── Input ────────────────────────────────────────────────────────────────────

ORACLE_ADDR_HEX="${ORACLE_ADDR_HEX:-${1:-}}"

# ─── Verdict tracking ─────────────────────────────────────────────────────────

FAIL_REASONS=()
fail() { FAIL_REASONS+=("$1"); }

# ─── Helpers ──────────────────────────────────────────────────────────────────

header() {
  printf '\n=== %s ===\n' "$1"
}

# Fire a JSON-RPC call. Returns the raw response body or empty on failure.
# Args: port method params_json
rpc_call() {
  local port="$1" method="$2" params="$3"
  curl -s --max-time "$CURL_TIMEOUT" \
    -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"${method}\",\"params\":${params},\"id\":1}" \
    "http://localhost:${port}" 2>/dev/null || true
}

# Extract a top-level .result.<field> from a JSON-RPC response. Empty on miss.
rpc_field() {
  local response="$1" field="$2"
  echo "$response" | jq -r ".result.${field} // empty" 2>/dev/null || true
}

# ─── Environment preflight ────────────────────────────────────────────────────

for tool in curl jq journalctl systemctl df; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FATAL: required tool not found: $tool" >&2
    exit 2
  fi
done

# Confirm @0 RPC is responsive before doing per-validator work. If not, the
# evidence is going to be too thin to be useful; bail with exit 2 so the
# operator knows this is an environment problem, not a divergence.
if [[ -z "$(rpc_call 3030 novai_getLatestBlock '[]')" ]]; then
  echo "FATAL: novai_getLatestBlock on port 3030 (@0) returned nothing" >&2
  echo "Check: systemctl is-active novai-node@0; curl -v http://localhost:3030" >&2
  exit 2
fi

# ─── Banner ───────────────────────────────────────────────────────────────────

header "BUG1 VERIFY - $(date -u +'%Y-%m-%d %H:%M:%S')Z"
echo "host:               $(hostname)"
echo "uname:              $(uname -srm)"
echo "binary:             /usr/local/bin/novai-node"
echo "binary md5:         $(md5sum /usr/local/bin/novai-node 2>/dev/null | awk '{print $1}')"
echo "binary mtime:       $(stat -c %y /usr/local/bin/novai-node 2>/dev/null || stat -f %Sm /usr/local/bin/novai-node 2>/dev/null)"
echo "oracle addr input:  ${ORACLE_ADDR_HEX:-<not provided>}"

# ─── 1. Systemd unit states ───────────────────────────────────────────────────

header "1. SYSTEMD UNIT STATES"
for svc in "${SERVICES[@]}"; do
  # is-active exits non-zero for non-active states; tolerate that.
  state="$(systemctl is-active "$svc" 2>/dev/null || true)"
  state="${state:-unknown}"
  printf '  %-24s %s\n' "$svc" "$state"

  case "$svc" in
    novai-node@*)
      if [[ "$state" != "active" ]]; then
        fail "$svc is $state (expected active)"
      fi
      ;;
    novai-monitor|novai-price-oracle)
      # Inactive monitor/oracle is not a verdict failure - note for the
      # operator but don't fail the script. Pre-G.4 runs of this script will
      # see novai-price-oracle inactive by design.
      if [[ "$state" != "active" ]]; then
        echo "    (note: $svc is $state - not fatal, operator awareness only)"
      fi
      ;;
  esac
done

# ─── 2. Latest committed block per validator ──────────────────────────────────

header "2. LATEST COMMITTED BLOCK PER VALIDATOR"

declare -A LATEST_HEIGHT
declare -A LATEST_HASH
declare -A LATEST_STATE_ROOT

min_head=""
for i in "${!VALIDATOR_PORTS[@]}"; do
  port="${VALIDATOR_PORTS[$i]}"
  idx="${VALIDATOR_INDICES[$i]}"
  resp="$(rpc_call "$port" novai_getLatestBlock '[]')"

  if [[ -z "$resp" ]]; then
    printf '  @%-2s port=%s  UNREACHABLE\n' "$idx" "$port"
    LATEST_HEIGHT[$port]=""
    fail "@${idx} (port ${port}) novai_getLatestBlock unreachable"
    continue
  fi

  # Defensive: getLatestBlock returns null at height 0.
  h="$(rpc_field "$resp" height)"
  bh="$(rpc_field "$resp" block_hash)"
  sr="$(rpc_field "$resp" state_root)"

  if [[ -z "$h" || "$h" == "null" ]]; then
    printf '  @%-2s port=%s  HEIGHT=0 (no committed blocks)\n' "$idx" "$port"
    LATEST_HEIGHT[$port]=""
    fail "@${idx} (port ${port}) head height is 0"
    continue
  fi

  LATEST_HEIGHT[$port]="$h"
  LATEST_HASH[$port]="$bh"
  LATEST_STATE_ROOT[$port]="$sr"
  printf '  @%-2s port=%s  height=%-8s block_hash=%s...  state_root=%s...\n' \
    "$idx" "$port" "$h" "${bh:0:16}" "${sr:0:16}"

  if [[ -z "$min_head" || "$h" -lt "$min_head" ]]; then
    min_head="$h"
  fi
done

if [[ -z "$min_head" ]]; then
  echo "  (no responding validators - skipping state_root agreement)"
  fail "no responding validators for head reference"
  min_head=0
fi
echo "  min_head (reference for height checks): ${min_head}"

# ─── 3. Per-validator state_root at 4 heights ─────────────────────────────────

header "3. STATE ROOTS BY HEIGHT (head, head-10, head-100, head-1000)"

# Build the list of test heights, clamped to [1, min_head]. Dedup if min_head
# is small (e.g., shortly after a wipe min_head=50 collapses three offsets to 1).
declare -a TEST_HEIGHTS=()
declare -A SEEN_HEIGHT
for off in "${HEAD_OFFSETS[@]}"; do
  if (( min_head > off )); then
    h=$(( min_head - off ))
  else
    h=1
  fi
  if [[ -z "${SEEN_HEIGHT[$h]:-}" ]]; then
    TEST_HEIGHTS+=("$h")
    SEEN_HEIGHT[$h]=1
  fi
done

# STATE_ROOT_AT[height,port] populated for the verdict pass.
declare -A STATE_ROOT_AT

for h in "${TEST_HEIGHTS[@]}"; do
  printf '  height=%s\n' "$h"
  for i in "${!VALIDATOR_PORTS[@]}"; do
    port="${VALIDATOR_PORTS[$i]}"
    idx="${VALIDATOR_INDICES[$i]}"
    resp="$(rpc_call "$port" novai_getBlockByHeight "{\"height\":${h}}")"

    if [[ -z "$resp" ]]; then
      printf '    @%-2s port=%s  RPC UNREACHABLE\n' "$idx" "$port"
      STATE_ROOT_AT["${h},${port}"]="__UNREACH__"
      continue
    fi

    # Reject = error response. Display the error and mark unreachable for verdict.
    err="$(echo "$resp" | jq -r '.error.message // empty' 2>/dev/null || true)"
    if [[ -n "$err" ]]; then
      printf '    @%-2s port=%s  RPC ERROR: %s\n' "$idx" "$port" "$err"
      STATE_ROOT_AT["${h},${port}"]="__ERROR__"
      continue
    fi

    sr="$(rpc_field "$resp" state_root)"
    if [[ -z "$sr" || "$sr" == "null" ]]; then
      printf '    @%-2s port=%s  state_root=<absent>\n' "$idx" "$port"
      STATE_ROOT_AT["${h},${port}"]="__ABSENT__"
      continue
    fi
    STATE_ROOT_AT["${h},${port}"]="$sr"
    printf '    @%-2s port=%s  state_root=%s\n' "$idx" "$port" "$sr"
  done
done

# ─── 4. AGREE / DIVERGE verdict per height ────────────────────────────────────

header "4. STATE ROOT VERDICT PER HEIGHT"

# Agreement is decided by MAJORITY across responding validators, not by
# first-responder comparison. With a 3-1 split where the first responder
# is the dissenter, the dissenting port is the lone one, not the three
# agreeing ones. Even splits (e.g. 2-2) are reported as SPLIT (no
# majority) and fail explicitly. Ported from scripts/verify-local-devnet.sh.

for h in "${TEST_HEIGHTS[@]}"; do
  any_unreach=0

  # Per-height parallel arrays of responding (port, state_root) pairs.
  resp_ports=()
  resp_roots=()
  for i in "${!VALIDATOR_PORTS[@]}"; do
    port="${VALIDATOR_PORTS[$i]}"
    sr="${STATE_ROOT_AT["${h},${port}"]}"
    case "$sr" in
      __UNREACH__|__ERROR__|__ABSENT__)
        any_unreach=1
        continue
        ;;
    esac
    resp_ports[${#resp_ports[@]}]="$port"
    resp_roots[${#resp_roots[@]}]="$sr"
  done

  # Group responders by state_root. Parallel arrays:
  #   group_roots[g]  the gth unique state_root
  #   group_counts[g] how many responders hold that root
  #   group_ports[g]  space-separated port list for that root
  group_roots=()
  group_counts=()
  group_ports=()
  for k in "${!resp_roots[@]}"; do
    r="${resp_roots[$k]}"
    p="${resp_ports[$k]}"
    found=-1
    for g in "${!group_roots[@]}"; do
      if [[ "${group_roots[$g]}" == "$r" ]]; then
        found=$g
        break
      fi
    done
    if (( found >= 0 )); then
      group_counts[$found]=$((group_counts[$found] + 1))
      group_ports[$found]="${group_ports[$found]} $p"
    else
      n=${#group_roots[@]}
      group_roots[$n]="$r"
      group_counts[$n]=1
      group_ports[$n]="$p"
    fi
  done

  n_groups=${#group_roots[@]}

  if (( n_groups == 0 )); then
    printf '  height=%-8s NO DATA (every validator unreachable or absent)\n' "$h"
    fail "height ${h}: no data from any validator"
    continue
  fi

  if (( n_groups == 1 )); then
    if (( any_unreach == 1 )); then
      printf '  height=%-8s PARTIAL (some validators unreachable; responders AGREE on %s...)\n' \
        "$h" "${group_roots[0]:0:16}"
      fail "height ${h}: partial verdict, unreachable validator(s)"
    else
      printf '  height=%-8s AGREE state_root=%s\n' "$h" "${group_roots[0]}"
    fi
    continue
  fi

  # n_groups >= 2: divergence. Find the largest group and check for a tie.
  max_count=0
  max_idx=-1
  for g in "${!group_counts[@]}"; do
    c=${group_counts[$g]}
    if (( c > max_count )); then
      max_count=$c
      max_idx=$g
    fi
  done
  tied=0
  for g in "${!group_counts[@]}"; do
    if (( g != max_idx )) && (( group_counts[$g] == max_count )); then
      tied=1
    fi
  done

  if (( tied == 1 )); then
    # No clear majority. Report SPLIT and dump every group.
    printf '  height=%-8s SPLIT (no majority): %d distinct state_roots across %d responders\n' \
      "$h" "$n_groups" "${#resp_roots[@]}"
    for g in "${!group_roots[@]}"; do
      printf '    GROUP %d  count=%s  ports=[%s]  state_root=%s\n' \
        "$g" "${group_counts[$g]}" "${group_ports[$g]}" "${group_roots[$g]}"
    done
    fail "height ${h}: SPLIT (no majority) across ${#resp_roots[@]} responders"
  else
    # Clear majority. Print majority + dissenters.
    maj_root="${group_roots[$max_idx]}"
    maj_ports="${group_ports[$max_idx]}"
    printf '  height=%-8s DIVERGE majority_root=%s... (count=%d, ports=[%s])\n' \
      "$h" "${maj_root:0:16}" "$max_count" "$maj_ports"
    diss_ports_all=""
    for g in "${!group_roots[@]}"; do
      if (( g == max_idx )); then continue; fi
      dp="${group_ports[$g]}"
      printf '    DISSENT  count=%s  ports=[%s]  state_root=%s\n' \
        "${group_counts[$g]}" "$dp" "${group_roots[$g]}"
      if [[ -z "$diss_ports_all" ]]; then
        diss_ports_all="$dp"
      else
        diss_ports_all="$diss_ports_all $dp"
      fi
    done
    fail "height ${h}: DIVERGE dissenting_ports=[${diss_ports_all}] majority_root=${maj_root:0:16}..."
  fi
done

# ─── 5. Oracle entity record ──────────────────────────────────────────────────

header "5. ORACLE ENTITY RECORD"

if [[ -z "$ORACLE_ADDR_HEX" ]]; then
  echo "  ORACLE_ADDR_HEX not provided - skipping oracle section."
  echo "  (Pass as env var or first positional arg. Address comes from"
  echo "   docs/deploy-bug1-fix.md Section G.2 after bootstrap.py runs.)"
elif [[ ! -f "$ORACLE_KEYFILE" ]]; then
  echo "  ORACLE_ADDR_HEX=${ORACLE_ADDR_HEX}"
  echo "  Keyfile ${ORACLE_KEYFILE} not present - oracle bootstrap has not run."
elif ! jq -e '.entity_id_hex' "$ORACLE_KEYFILE" >/dev/null 2>&1; then
  echo "  ORACLE_ADDR_HEX=${ORACLE_ADDR_HEX}"
  echo "  Keyfile present but entity_id_hex absent - bootstrap generated key but"
  echo "  did not complete registration. Check /var/log/novai-price-oracle for"
  echo "  bootstrap exit code (per docs/deploy-bug1-fix.md Section G.2)."
else
  keyfile_addr="$(jq -r '.address_hex' "$ORACLE_KEYFILE" 2>/dev/null || true)"
  entity_id="$(jq -r '.entity_id_hex' "$ORACLE_KEYFILE")"
  caps_byte="$(jq -r '.capabilities_byte // empty' "$ORACLE_KEYFILE")"

  echo "  ORACLE_ADDR_HEX (input):       ${ORACLE_ADDR_HEX}"
  echo "  address_hex (keyfile):         ${keyfile_addr}"
  if [[ "$keyfile_addr" != "$ORACLE_ADDR_HEX" ]]; then
    echo "  WARNING: input address does NOT match keyfile address - using keyfile."
  fi
  echo "  entity_id_hex (keyfile):       ${entity_id}"
  echo "  capabilities_byte (keyfile):   ${caps_byte} (expect 71 / 0x47 for oracle)"

  resp="$(rpc_call 3030 novai_getAiEntity "{\"entity_id\":\"${entity_id}\"}")"
  if [[ -z "$resp" ]]; then
    echo "  novai_getAiEntity RPC unreachable on port 3030."
  else
    err="$(echo "$resp" | jq -r '.error.message // empty' 2>/dev/null || true)"
    if [[ -n "$err" ]]; then
      echo "  novai_getAiEntity error: $err"
    else
      nonce="$(rpc_field "$resp" nonce)"
      is_active="$(rpc_field "$resp" is_active)"
      caps_onchain="$(rpc_field "$resp" capabilities)"
      total_tx="$(rpc_field "$resp" total_transactions)"
      reputation="$(rpc_field "$resp" reputation_score)"
      last_active="$(rpc_field "$resp" last_active_at)"
      echo "  ── on-chain (novai_getAiEntity) ──"
      echo "    nonce:                       ${nonce:-<absent>}"
      echo "    is_active:                   ${is_active:-<absent>}"
      echo "    capabilities (on-chain):     ${caps_onchain:-<absent>}"
      echo "    total_transactions:          ${total_tx:-<absent>}"
      echo "    reputation_score:            ${reputation:-<absent>}"
      echo "    last_active_at:              ${last_active:-<absent>}"
    fi
  fi
fi

# ─── 6. Disk usage ────────────────────────────────────────────────────────────

header "6. DISK USAGE"
df -h / /var/lib/novai 2>/dev/null || df -h /

# ─── 7. Last 20 journal lines for novai-node@0 ────────────────────────────────

header "7. JOURNAL - novai-node@0 (last 20 lines)"
journalctl -u novai-node@0 -n 20 --no-pager 2>&1 || echo "  (journalctl failed)"

# ─── 8. State root mismatch grep - last 1 hour across all 4 validators ────────

header "8. STATE ROOT MISMATCH GREP - last 1 hour, all 4 validators"
# journalctl --since accepts free-form time; -i grep so 'State root mismatch' and
# 'state_root mismatch' both match.
mismatch_lines="$(journalctl --since '1 hour ago' --no-pager \
    -u 'novai-node@0' -u 'novai-node@1' -u 'novai-node@2' -u 'novai-node@3' \
    2>/dev/null | grep -i 'state root mismatch\|state_root mismatch' || true)"

if [[ -z "$mismatch_lines" ]]; then
  echo "  (no matches - clean)"
else
  mismatch_count="$(printf '%s\n' "$mismatch_lines" | wc -l | tr -d ' ')"
  echo "  ${mismatch_count} mismatch line(s) found in last hour:"
  printf '%s\n' "$mismatch_lines" | sed 's/^/    /'
  fail "found ${mismatch_count} 'State root mismatch' line(s) in last hour"
fi

# ─── Final verdict ────────────────────────────────────────────────────────────

header "FINAL VERDICT"
if (( ${#FAIL_REASONS[@]} == 0 )); then
  echo "  PASS - all 4 validators AGREE at all tested heights and no State root mismatch lines."
  echo "  Heights tested: ${TEST_HEIGHTS[*]}"
  exit 0
else
  echo "  FAIL - verdict failed on ${#FAIL_REASONS[@]} condition(s):"
  for reason in "${FAIL_REASONS[@]}"; do
    echo "    - ${reason}"
  done
  echo
  echo "  Next step: paste this entire output to claude.ai for review."
  echo "  Do NOT redeploy or re-wipe based on these symptoms alone."
  exit 1
fi
