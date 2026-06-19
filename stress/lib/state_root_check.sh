#!/bin/bash
# stress/lib/state_root_check.sh
# Cross-validator state-root agreement check (fork detection).
#
# Method: query each validator's committed state_root at a COMMON height, group
# the reported roots by value, and take the MAJORITY group as the reference. If
# any two responding nodes report different roots at the same height, that is a
# fork: the check fails loud and names the minority nodes as the dissenters.
#
# It never compares every node to a single fixed reference node. If that fixed
# reference were itself the forked node, compare-to-first would mislabel the
# honest majority as wrong. Grouping by value and taking the majority labels the
# true dissenter regardless of node order.
#
# The verdict logic is pure (it reads "label|root" pairs on stdin) so it is
# provable offline via --self-test, with no devnet required.

_SR_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
. "$_SR_DIR/common.sh"

# Short display form of a root (first 16 hex chars); full value used for compares.
_sr_short() {
  local r="$1"
  if [ "${#r}" -gt 16 ]; then
    printf '%s...' "${r:0:16}"
  else
    printf '%s' "$r"
  fi
}

# sr_verdict_from_pairs <height>
#   Reads "label|root" lines on stdin (one per responding node).
#   Prints a human report plus machine-checkable MAJORITY: and DISSENTERS: lines.
#   Returns: 0 = agreement, 1 = fork detected, 2 = cannot verify (no responders).
sr_verdict_from_pairs() {
  local height="$1"
  local labels=() roots=()
  local line label root
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    # Only accept a well-formed "label|root" reading. A line with no '|'
    # separator is not a node root (a diagnostic that leaked onto the
    # collection stream, or an absent/unreached node): skip it so it stays
    # excluded from the comparison, never carried into the roots array as a
    # bogus value that would masquerade as a dissenter.
    case "$line" in
      *'|'*) ;;
      *) continue ;;
    esac
    label="${line%%|*}"
    root="${line#*|}"
    [ -z "$label" ] && continue
    [ -z "$root" ] && continue
    labels+=("$label")
    roots+=("$root")
  done

  local n="${#roots[@]}"
  if [ "$n" -eq 0 ]; then
    log_error "state_root agreement at height $height: no nodes responded, cannot verify"
    printf 'MAJORITY: NONE\nDISSENTERS: NONE\n'
    return 2
  fi

  # Count distinct roots.
  local distinct distinct_count
  distinct="$(printf '%s\n' "${roots[@]}" | sort -u)"
  distinct_count="$(printf '%s\n' "$distinct" | grep -c .)"

  if [ "$distinct_count" -eq 1 ]; then
    log_ok "state_root agreement at height $height: all $n node(s) share root $(_sr_short "${roots[0]}")"
    printf 'MAJORITY: %s\nDISSENTERS: NONE\n' "${roots[0]}"
    return 0
  fi

  # Fork. Rank roots by frequency to find the majority group.
  local ranked top_count top_root second_count
  ranked="$(printf '%s\n' "${roots[@]}" | sort | uniq -c | sort -rn)"
  top_count="$(printf '%s\n' "$ranked" | head -n1 | awk '{print $1}')"
  top_root="$(printf '%s\n' "$ranked" | head -n1 | awk '{print $2}')"
  second_count="$(printf '%s\n' "$ranked" | sed -n '2p' | awk '{print $1}')"

  local majority_root="$top_root" majority_note=""
  if [ -n "$second_count" ] && [ "$second_count" -eq "$top_count" ]; then
    majority_root=""   # no clear majority (the top two groups are tied)
    majority_note=" (no clear majority, tied split)"
  fi

  log_fail "FORK DETECTED at height $height: nodes disagree on state_root${majority_note}"

  local i
  for i in $(seq 0 $(( n - 1 ))); do
    log_error "  ${labels[$i]}: $(_sr_short "${roots[$i]}")"
  done

  local maj_nodes="" dis_nodes=""
  if [ -n "$majority_root" ]; then
    for i in $(seq 0 $(( n - 1 ))); do
      if [ "${roots[$i]}" = "$majority_root" ]; then
        maj_nodes="${maj_nodes:+$maj_nodes,}${labels[$i]}"
      else
        dis_nodes="${dis_nodes:+$dis_nodes,}${labels[$i]}"
      fi
    done
    log_error "  majority root: $(_sr_short "$majority_root") on [${maj_nodes}]"
    log_error "  dissenters: [${dis_nodes}]"
    printf 'MAJORITY: %s\nDISSENTERS: %s\n' "$majority_root" "$dis_nodes"
  else
    # Tied split: no honest majority can be named; every node is in conflict.
    for i in $(seq 0 $(( n - 1 ))); do
      dis_nodes="${dis_nodes:+$dis_nodes,}${labels[$i]}"
    done
    log_error "  no majority; conflicting nodes: [${dis_nodes}]"
    printf 'MAJORITY: NONE\nDISSENTERS: %s\n' "$dis_nodes"
  fi
  return 1
}

# sr_collect_pairs <height>
#   Queries each configured node for its state_root at <height> and emits
#   "label|root" lines for nodes that have reached that height.
sr_collect_pairs() {
  local height="$1" i root
  for i in $(seq 0 $(( STRESS_NODES - 1 ))); do
    if root="$(rpc_state_root_at_height "$(node_rpc_url "$i")" "$height")" && [ -n "$root" ]; then
      printf 'node%s|%s\n' "$i" "$root"
    else
      log_warn "node$i has not reached height $height (or RPC unavailable); excluded from this check"
    fi
  done
}

# state_root_agreement <height>
#   Live check across the configured local cluster. Returns the verdict code
#   (0 agree, 1 fork, 2 cannot verify).
state_root_agreement() {
  local height="$1"
  sr_collect_pairs "$height" | sr_verdict_from_pairs "$height"
}

# --- Self-test (no devnet required) ---------------------------------------

_SR_FAILS=0

# _sr_case <name> <want_rc> <want_dissenters_csv|-> <pairs-newline-string>
_sr_case() {
  local name="$1" want_rc="$2" want_dis="$3" pairs="$4"
  local out rc dis
  out="$(printf '%s\n' "$pairs" | sr_verdict_from_pairs 99 2>&1)"
  rc=$?
  dis="$(printf '%s\n' "$out" | grep '^DISSENTERS:' | head -n1 | sed 's/^DISSENTERS: //')"
  if [ "$rc" -ne "$want_rc" ]; then
    log_fail "self-test [$name]: expected rc=$want_rc, got rc=$rc"
    _SR_FAILS=$(( _SR_FAILS + 1 ))
    return
  fi
  if [ "$want_dis" != "-" ] && [ "$dis" != "$want_dis" ]; then
    log_fail "self-test [$name]: expected dissenters=[$want_dis], got [$dis]"
    _SR_FAILS=$(( _SR_FAILS + 1 ))
    return
  fi
  log_ok "self-test [$name]: rc=$rc dissenters=[$dis] as expected"
}

sr_self_test() {
  _SR_FAILS=0
  log_info "Running state_root_check self-test (no devnet required)..."

  # Case 1: all four agree -> agreement (rc 0).
  _sr_case "all-agree" 0 "NONE" \
"node0|aaaa
node1|aaaa
node2|aaaa
node3|aaaa"

  # Case 2: single dissenter at the tail -> fork (rc 1), dissenter node3.
  _sr_case "one-dissenter-tail" 1 "node3" \
"node0|aaaa
node1|aaaa
node2|aaaa
node3|bbbb"

  # Case 3: dissenter is node0 -> fork (rc 1), dissenter node0.
  # This is the critical case: it proves majority grouping, not compare-to-first.
  _sr_case "dissenter-is-node0" 1 "node0" \
"node0|bbbb
node1|aaaa
node2|aaaa
node3|aaaa"

  # Case 4: tied split -> fork (rc 1), no clear majority, all nodes conflicting.
  _sr_case "tied-split" 1 "node0,node1,node2,node3" \
"node0|aaaa
node1|aaaa
node2|bbbb
node3|bbbb"

  # Case 5: only a subset responded but they all agree -> agreement (rc 0).
  _sr_case "subset-agree" 0 "NONE" \
"node0|aaaa
node2|aaaa"

  # Case 6: a stray non-pair line (a diagnostic that leaked onto the collection
  # stream, or an absent/unreached node) must be excluded entirely, not carried
  # in as a bogus root. The reachable nodes agree -> agreement (rc 0), NO
  # dissenters. Regression guard for the kill-node false fork, where an absent
  # victim was being counted as a dissenter.
  _sr_case "stray-line-excluded" 0 "NONE" \
"node0|aaaa
[WARN] node1 has not reached height 99 (or RPC unavailable); excluded from this check
node2|aaaa
node3|aaaa"

  echo
  if [ "$_SR_FAILS" -eq 0 ]; then
    log_ok "state_root_check self-test: ALL CASES PASSED"
    return 0
  fi
  log_fail "state_root_check self-test: $_SR_FAILS case(s) FAILED"
  return 1
}

# --- CLI -------------------------------------------------------------------
# Runs only when executed directly, not when sourced by a scenario.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  set -uo pipefail
  case "${1:-}" in
    --self-test)
      sr_self_test
      ;;
    --height)
      shift
      h="${1:-}"
      if ! is_uint "$h"; then
        log_error "Usage: state_root_check.sh --height <committed-height>"
        exit 2
      fi
      stress_preflight
      state_root_agreement "$h"
      ;;
    *)
      printf 'Usage: %s {--self-test | --height <N>}\n' "$0"
      exit 2
      ;;
  esac
fi
