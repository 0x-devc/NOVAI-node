#!/bin/bash
# stress/scenarios/load.sh
# Load scenario: drive the tx-generator against a local full-mesh devnet while
# concurrently asserting the chain keeps committing and agreeing under load.
#
# Concurrently with the load it runs the same invariant sampling as the soak
# (no height regression, forward progress, bounded round, peer_count == N-1,
# cross-validator state_root agreement at a common height), plus load-side
# assertions parsed from the tx-generator JSON output (the generator actually
# submitted transactions and the chain accepted them under load).
#
# FUNDING PRECONDITION: the tx-generator has no faucet; it derives sender keys
# deterministically (SenderAccount::from_index) and expects them pre-funded. The
# dev-keys genesis (apply_dev_genesis) funds the first 100 of those exact
# addresses (indices 0..99) at 1e9 each, using the identical derivation and the
# same address_from_pubkey. So --senders must stay at or below 100 (default 10);
# this is enforced in load_check_preconditions.
#
# Safety: local devnet only (127.0.0.1). High-rate load (tps above
# STRESS_TPS_SAFE_CAP) requires --enable-destructive. --dry-run validates the
# logic, preconditions, and command construction without standing up a cluster
# or generating load.

set -uo pipefail

_LOAD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
. "$_LOAD_DIR/../lib/common.sh"
# shellcheck source=../lib/assert.sh
. "$_LOAD_DIR/../lib/assert.sh"
# shellcheck source=../lib/state_root_check.sh
. "$_LOAD_DIR/../lib/state_root_check.sh"
# shellcheck source=../lib/cluster.sh
. "$_LOAD_DIR/../lib/cluster.sh"

LOAD_MANAGE=1
LOAD_DRYRUN=0
LOAD_REPORT=""

usage() {
  cat <<EOF
load: drive the tx-generator against a local full-mesh devnet and assert the
chain keeps committing and agreeing under load.

Usage: $0 [flags]

Flags:
  --dry-run             Validate logic, preconditions, and commands (no cluster, no load).
  --attach              Use an already-running cluster (do not start or stop it).
  --tps <n>             Target transactions per second (default ${STRESS_TPS}).
  --senders <n>         Sender accounts, max ${STRESS_DEV_FUNDED_ACCOUNTS} (default ${STRESS_SENDERS}).
  --duration <s>        Load and sampling duration in seconds (default ${STRESS_DURATION}).
  --tx-type <t>         transfer | ai_register | ai_signal (default ${STRESS_TX_TYPE}).
  --workers <n>         Submitter workers (default ${STRESS_WORKERS}).
  --node <i>            Which node's RPC to drive (default ${STRESS_LOAD_NODE}).
  --interval <s>        Sample interval in seconds (default ${STRESS_INTERVAL}).
  --round-max <n>       Maximum allowed consensus round (default ${STRESS_ROUND_MAX}).
  --nodes <n>           Validator count (default ${STRESS_NODES}); sets expected peers to n-1.
  --enable-destructive  Allow tps above the safe cap (${STRESS_TPS_SAFE_CAP}).
  --report <file>       Write a plain-text report copy to this file.
  -h, --help            Show this help.
EOF
}

# Smallest of a list of integers.
_min_of() {
  local m="$1"; shift
  local v
  for v in "$@"; do [ "$v" -lt "$m" ] && m="$v"; done
  printf '%s' "$m"
}

# Build the tx-generator argv. Endpoint is a local node RPC only.
load_txgen_argv() {
  local node="${1:-$STRESS_LOAD_NODE}"
  assert_localhost_or_die "$(node_rpc_url "$node")" "load endpoint"
  LOAD_ARGV=(
    "$STRESS_TXGEN_BIN"
    --tps "$STRESS_TPS"
    --senders "$STRESS_SENDERS"
    --duration "$STRESS_DURATION"
    --tx-type "$STRESS_TX_TYPE"
    --endpoint "$(node_rpc_url "$node")"
    --workers "$STRESS_WORKERS"
    --output json
  )
}

# Enforce the funding and high-rate preconditions. Returns nonzero if violated.
load_check_preconditions() {
  if ! is_uint "$STRESS_SENDERS" || [ "$STRESS_SENDERS" -lt 1 ]; then
    log_error "invalid --senders: $STRESS_SENDERS"; return 1
  fi
  if [ "$STRESS_SENDERS" -gt "$STRESS_DEV_FUNDED_ACCOUNTS" ]; then
    log_error "senders=$STRESS_SENDERS exceeds the $STRESS_DEV_FUNDED_ACCOUNTS dev-genesis funded accounts (indices 0..$(( STRESS_DEV_FUNDED_ACCOUNTS - 1 )))."
    log_error "Unfunded senders would fail on insufficient balance; lower --senders."
    return 1
  fi
  if ! is_uint "$STRESS_TPS" || [ "$STRESS_TPS" -lt 1 ]; then
    log_error "invalid --tps: $STRESS_TPS"; return 1
  fi
  if [ "$STRESS_TPS" -gt "$STRESS_TPS_SAFE_CAP" ] && [ "$STRESS_ENABLE_DESTRUCTIVE" != "1" ]; then
    log_error "tps=$STRESS_TPS exceeds safe cap $STRESS_TPS_SAFE_CAP; high-rate load requires --enable-destructive (default off)."
    return 1
  fi
  return 0
}

# Verify the tx-generator binary exists.
load_build_check() {
  if [ ! -x "$STRESS_TXGEN_BIN" ]; then
    log_error "tx-generator binary not found or not executable: $STRESS_TXGEN_BIN"
    log_error "Build it with: cargo build --release -p tx-generator"
    exit 5
  fi
}

# Extract the single JSON result line from the tx-generator stdout (the result is
# printed with println! while tracing logs share stdout; the JSON line is the one
# carrying the "submitted_count" key).
load_extract_json() { grep '"submitted_count"' "$1" 2>/dev/null | tail -n1; }

# Assert the load actually applied and the chain accepted it. Records into the
# tally and returns nonzero if any load assertion failed.
load_assert_results() {
  local j="$1" sub acc rej fail bad=0
  sub="$(printf '%s' "$j" | jq -r '.submitted_count // empty' 2>/dev/null || true)"
  acc="$(printf '%s' "$j" | jq -r '.accepted_count // empty' 2>/dev/null || true)"
  rej="$(printf '%s' "$j" | jq -r '.rejected_count // empty' 2>/dev/null || true)"
  fail="$(printf '%s' "$j" | jq -r '.failed_count // empty' 2>/dev/null || true)"
  if ! is_uint "$sub" || ! is_uint "$acc"; then
    record_fail "load_results_parsed" "could not parse tx-generator JSON output"
    return 1
  fi
  if [ "$sub" -lt 1 ]; then
    record_fail "load_generated" "tx-generator submitted 0 transactions"; bad=1
  else
    record_pass "load_generated" "submitted=$sub"
  fi
  if [ "$acc" -lt 1 ]; then
    record_fail "load_accepted" "chain accepted 0 transactions under load (rejected=${rej:-?} failed=${fail:-?})"; bad=1
  else
    record_pass "load_accepted" "accepted=$acc (rejected=${rej:-?} failed=${fail:-?})"
  fi
  return "$bad"
}

# One sampling pass over all nodes (mirrors the soak invariants). bash 3.2 has no
# namerefs, so per-node previous heights are shared via the global LOAD_PREV_H,
# and this pass publishes LOAD_HEIGHTS. Returns 2 if a fork is observed.
load_sample_pass() {
  local i h r p
  LOAD_HEIGHTS=()
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    r="$(get_node_metric "$i" novai_current_round 2>/dev/null || true)"
    p="$(get_node_metric "$i" novai_peer_count 2>/dev/null || true)"
    if ! is_uint "$h"; then
      record_fail "node_reachable[node$i]" "metrics unreadable (committed_height='$h')"
      continue
    fi
    assert_no_height_regression "${LOAD_PREV_H[$i]}" "$h" "node$i"
    assert_round_bounded "$r" "$STRESS_ROUND_MAX" "node$i"
    assert_peer_count_eq "$p" "$STRESS_EXPECTED_PEERS" "node$i"
    LOAD_PREV_H[$i]="$h"
    LOAD_HEIGHTS[$i]="$h"
  done
  if [ "${#LOAD_HEIGHTS[@]}" -eq "$STRESS_NODES" ]; then
    local min_h target sr_rc
    min_h="$(_min_of "${LOAD_HEIGHTS[@]}")"
    target=$(( min_h - STRESS_SAMPLE_MARGIN ))
    if [ "$target" -ge 1 ]; then
      sr_rc=0
      state_root_agreement "$target" || sr_rc=$?
      if [ "$sr_rc" -eq 0 ]; then
        record_pass "state_root_agreement@$target" "all nodes agree"
      else
        record_fail "state_root_agreement@$target" "divergence or unverifiable (rc=$sr_rc)"
        [ "$sr_rc" -eq 1 ] && return 2
      fi
    fi
  fi
  return 0
}

load_print_plan() {
  log_info "Plan: load against ${STRESS_NODES}-node full-mesh local devnet on 127.0.0.1"
  log_info "  tps=${STRESS_TPS} senders=${STRESS_SENDERS} tx_type=${STRESS_TX_TYPE} workers=${STRESS_WORKERS} duration=${STRESS_DURATION}s"
  log_info "  load endpoint: node ${STRESS_LOAD_NODE} rpc $(node_rpc_url "$STRESS_LOAD_NODE")"
  log_info "  funding: dev-genesis funds sender indices 0..$(( STRESS_DEV_FUNDED_ACCOUNTS - 1 )) at 1e9 each (no faucet)"
  load_txgen_argv "$STRESS_LOAD_NODE"
  log_info "  txgen: ${LOAD_ARGV[*]}"
}

# Verify the cluster launcher builds a local-only full-mesh command for each node.
load_dry_check_launcher() {
  local i bad=0 joined peers
  for (( i = 0; i < STRESS_NODES; i++ )); do
    cluster_build_argv "$i"
    joined="${CLUSTER_ARGV[*]}"
    if printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' \
         | grep -vq '^127\.0\.0\.1$'; then
      log_fail "launcher node$i: non-local address present: $joined"; bad=1
    fi
    peers="$(printf '%s\n' "${CLUSTER_ARGV[@]}" | grep -c '^--peer$' || true)"
    if [ "$peers" -ne "$i" ]; then
      log_fail "launcher node$i: expected $i --peer flags (full mesh), got $peers"; bad=1
    fi
  done
  if [ "$bad" -eq 0 ]; then log_ok "launcher: every node builds a local-only full-mesh command"; return 0; fi
  return 1
}

# Verify the tx-generator command targets a local endpoint with JSON output.
load_dry_check_txgen() {
  load_txgen_argv "$STRESS_LOAD_NODE"
  local joined ep
  joined="${LOAD_ARGV[*]}"
  ep="$(printf '%s\n' "${LOAD_ARGV[@]}" | grep -A1 -- '--endpoint' | tail -n1)"
  if ! is_localhost "$ep"; then
    log_fail "txgen endpoint is not local: $ep"; return 1
  fi
  case "$joined" in
    *"--output json"*) : ;;
    *) log_fail "txgen command missing --output json"; return 1 ;;
  esac
  log_ok "txgen command targets a local endpoint with JSON output"
  return 0
}

load_dry_run() {
  log_info "DRY RUN: validating load logic, preconditions, and commands (no cluster, no load)."
  load_print_plan
  echo
  local fails=0

  _expect() { # _expect <want_rc> <desc> <cmd...>
    local want="$1" desc="$2"; shift 2
    local rc=0
    "$@" >/dev/null 2>&1 || rc=$?
    if [ "$rc" -eq "$want" ]; then
      log_ok "dry-check: $desc (rc=$rc)"
    else
      log_fail "dry-check: $desc (want rc=$want, got $rc)"
      fails=$(( fails + 1 ))
    fi
  }
  _dry_sr() { printf '%s\n' "$1" | sr_verdict_from_pairs 5; }
  _dry_precond() { STRESS_SENDERS="$1" STRESS_TPS="$2" STRESS_ENABLE_DESTRUCTIVE="$3" load_check_preconditions; }

  # Invariant primitives (same as the soak, exercised under load).
  _expect 0 "no-regression accepts a climb" assert_no_height_regression 10 12 dry
  _expect 1 "no-regression catches a regression" assert_no_height_regression 12 9 dry
  _expect 0 "round within bound" assert_round_bounded 3 20 dry
  _expect 1 "round over bound" assert_round_bounded 99 20 dry
  _expect 0 "peer_count full mesh accepted" assert_peer_count_eq 3 3 dry
  _expect 1 "peer_count degraded caught" assert_peer_count_eq 1 3 dry
  _expect 0 "state_root agreement passes" _dry_sr "node0|aa
node1|aa"
  _expect 1 "state_root fork caught" _dry_sr "node0|aa
node1|bb"

  # Load-result parsing and assertions.
  _expect 0 "load results good" load_assert_results '{"submitted_count":1000,"accepted_count":950,"rejected_count":40,"failed_count":10}'
  _expect 1 "load zero-accept caught" load_assert_results '{"submitted_count":1000,"accepted_count":0,"rejected_count":900,"failed_count":100}'
  _expect 1 "load unparseable caught" load_assert_results 'not json at all'

  # Funding and high-rate preconditions.
  _expect 0 "safe load allowed" _dry_precond 10 100 0
  _expect 1 "senders over funded refused" _dry_precond 101 100 0
  _expect 1 "high tps gated off" _dry_precond 10 5000 0
  _expect 0 "high tps allowed when destructive" _dry_precond 10 5000 1

  # Local-only command construction.
  if ! load_dry_check_launcher; then fails=$(( fails + 1 )); fi
  if ! load_dry_check_txgen; then fails=$(( fails + 1 )); fi

  echo
  if [ "$fails" -eq 0 ]; then
    log_ok "load dry-run: logic, preconditions, and commands validated"
    return 0
  fi
  log_fail "load dry-run: $fails check(s) failed"
  return 1
}

load_live() {
  stress_preflight
  load_build_check
  if ! load_check_preconditions; then exit 6; fi

  local i h
  for (( i = 0; i < STRESS_NODES; i++ )); do
    assert_localhost_or_die "$(node_rpc_url "$i")" "rpc endpoint"
    assert_localhost_or_die "$(node_metrics_url "$i")" "metrics endpoint"
  done
  assert_localhost_or_die "$(node_rpc_url "$STRESS_LOAD_NODE")" "load endpoint"

  if [ "$LOAD_MANAGE" -eq 1 ]; then
    cluster_build_check
    cluster_start
    trap 'cluster_stop' EXIT INT TERM
    if ! cluster_wait_ready; then
      log_fail "cluster failed to become ready; aborting load"
      return 1
    fi
  else
    log_info "attach mode: using an already-running cluster (no start or stop)"
  fi

  assert_init
  local report="$LOAD_REPORT"
  if [ -z "$report" ]; then
    mkdir -p "$STRESS_REPORT_DIR" 2>/dev/null || true
    report="$STRESS_REPORT_DIR/load-$(date -u +%Y%m%dT%H%M%SZ).txt"
  fi

  # Initial per-node heights.
  LOAD_PREV_H=()
  local start_h=()
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" || h=0
    LOAD_PREV_H[$i]="$h"; start_h[$i]="$h"
  done

  # Start the load driver in the background, capturing its mixed stdout to a file.
  load_txgen_argv "$STRESS_LOAD_NODE"
  mkdir -p "$STRESS_LOG_DIR" 2>/dev/null || true
  local txgen_out="$STRESS_LOG_DIR/txgen-$(date -u +%Y%m%dT%H%M%SZ).log"
  log_info "Starting load: tps=${STRESS_TPS} senders=${STRESS_SENDERS} tx_type=${STRESS_TX_TYPE} workers=${STRESS_WORKERS} endpoint=$(node_rpc_url "$STRESS_LOAD_NODE") duration=${STRESS_DURATION}s"
  "${LOAD_ARGV[@]}" > "$txgen_out" 2>&1 &
  local txgen_pid=$!

  # Concurrently sample and assert for the duration.
  local start_ts now elapsed sp
  start_ts="$(date +%s)"
  log_info "Sampling invariants every ${STRESS_INTERVAL}s while the load runs..."
  while :; do
    load_sample_pass
    sp=$?
    if [ "$sp" -eq 2 ]; then
      log_fail "FORK observed during load; aborting sampling immediately"
      break
    fi
    now="$(date +%s)"; elapsed=$(( now - start_ts ))
    [ "$elapsed" -ge "$STRESS_DURATION" ] && break
    sleep "$STRESS_INTERVAL"
  done

  # Wait for the load driver to finish and collect its result.
  wait "$txgen_pid" 2>/dev/null
  local txgen_rc=$?
  local jsonline
  jsonline="$(load_extract_json "$txgen_out")"
  if [ -z "$jsonline" ]; then
    record_fail "load_driver" "tx-generator produced no JSON result (rc=$txgen_rc); see $txgen_out"
  else
    load_assert_results "$jsonline" || true
  fi

  # Forward progress across the window (slowest node at each end).
  local end_h=() smin emin
  for (( i = 0; i < STRESS_NODES; i++ )); do
    h="$(get_node_metric "$i" novai_committed_height 2>/dev/null || true)"
    is_uint "$h" || h="${LOAD_PREV_H[$i]}"
    end_h[$i]="$h"
  done
  smin="$(_min_of "${start_h[@]}")"
  emin="$(_min_of "${end_h[@]}")"
  assert_progress "$smin" "$emin" "$STRESS_MIN_PROGRESS" "cluster"

  assert_report "$report"
}

# --- Flag parsing ----------------------------------------------------------
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)            LOAD_DRYRUN=1 ;;
    --attach)             LOAD_MANAGE=0 ;;
    --tps)                shift; STRESS_TPS="${1:-}" ;;
    --senders)            shift; STRESS_SENDERS="${1:-}" ;;
    --duration)           shift; STRESS_DURATION="${1:-}" ;;
    --tx-type)            shift; STRESS_TX_TYPE="${1:-}" ;;
    --workers)            shift; STRESS_WORKERS="${1:-}" ;;
    --node)               shift; STRESS_LOAD_NODE="${1:-}" ;;
    --interval)           shift; STRESS_INTERVAL="${1:-}" ;;
    --round-max)          shift; STRESS_ROUND_MAX="${1:-}" ;;
    --nodes)              shift; STRESS_NODES="${1:-}"; STRESS_EXPECTED_PEERS=$(( STRESS_NODES - 1 )) ;;
    --enable-destructive) STRESS_ENABLE_DESTRUCTIVE=1 ;;
    --report)             shift; LOAD_REPORT="${1:-}" ;;
    -h|--help)            usage; exit 0 ;;
    *) log_error "unknown flag: $1"; usage; exit 2 ;;
  esac
  shift
done

if [ "$LOAD_DRYRUN" -eq 1 ]; then
  load_dry_run
  exit $?
fi
load_live
exit $?
