#!/bin/bash
# stress/lib/assert.sh
# Invariant assertion primitives plus pass/fail accounting and report emission.
# Every failed assertion is logged loudly; assert_report returns non-zero if any
# assertion failed, so scenarios are CI-friendly and fail loud on a real fault.

if [ -n "${STRESS_ASSERT_SOURCED:-}" ]; then
  return 0
fi
STRESS_ASSERT_SOURCED=1

_ASSERT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
. "$_ASSERT_DIR/common.sh"

# Accounting state.
ASSERT_PASS=0
ASSERT_FAIL=0
ASSERT_RESULTS=()   # entries: "PASS|name|detail" or "FAIL|name|detail"

assert_init() {
  ASSERT_PASS=0
  ASSERT_FAIL=0
  ASSERT_RESULTS=()
}

record_pass() {
  local name="$1" detail="${2:-}"
  ASSERT_PASS=$(( ASSERT_PASS + 1 ))
  ASSERT_RESULTS+=("PASS|${name}|${detail}")
  log_ok "${name}${detail:+: $detail}"
}

record_fail() {
  local name="$1" detail="${2:-}"
  ASSERT_FAIL=$(( ASSERT_FAIL + 1 ))
  ASSERT_RESULTS+=("FAIL|${name}|${detail}")
  log_fail "${name}${detail:+: $detail}"
}

# --- Invariant primitives -------------------------------------------------

# Height must never decrease (a regression is a safety violation).
# assert_no_height_regression <prev> <curr> <label>
assert_no_height_regression() {
  local prev="$1" curr="$2" label="${3:-height}"
  if ! is_uint "$prev" || ! is_uint "$curr"; then
    record_fail "height_readable[$label]" "non-numeric height (prev=$prev curr=$curr)"
    return 1
  fi
  if [ "$curr" -lt "$prev" ]; then
    record_fail "no_height_regression[$label]" "height went backward: $prev -> $curr"
    return 1
  fi
  record_pass "no_height_regression[$label]" "$prev -> $curr"
  return 0
}

# Height must make forward progress over a window.
# assert_progress <start> <end> <min_delta> <label>
assert_progress() {
  local start="$1" end="$2" min="$3" label="${4:-progress}"
  if ! is_uint "$start" || ! is_uint "$end"; then
    record_fail "progress_readable[$label]" "non-numeric height (start=$start end=$end)"
    return 1
  fi
  local delta=$(( end - start ))
  if [ "$delta" -lt "$min" ]; then
    record_fail "progress[$label]" "advanced $delta blocks, need at least $min ($start -> $end)"
    return 1
  fi
  record_pass "progress[$label]" "advanced $delta blocks ($start -> $end)"
  return 0
}

# Consensus round must stay within a bound.
# assert_round_bounded <round> <max> <label>
assert_round_bounded() {
  local round="$1" max="$2" label="${3:-round}"
  if ! is_uint "$round"; then
    record_fail "round_readable[$label]" "non-numeric round ($round)"
    return 1
  fi
  if [ "$round" -gt "$max" ]; then
    record_fail "round_bounded[$label]" "round $round exceeds bound $max"
    return 1
  fi
  record_pass "round_bounded[$label]" "round $round <= $max"
  return 0
}

# Peer count must equal the expected full-mesh value.
# assert_peer_count_eq <peers> <expected> <label>
assert_peer_count_eq() {
  local peers="$1" expected="$2" label="${3:-peers}"
  if ! is_uint "$peers"; then
    record_fail "peers_readable[$label]" "non-numeric peer count ($peers)"
    return 1
  fi
  if [ "$peers" -ne "$expected" ]; then
    record_fail "peer_count_eq[$label]" "peer_count $peers != expected $expected"
    return 1
  fi
  record_pass "peer_count_eq[$label]" "peer_count $peers"
  return 0
}

# Peer count must be at least a minimum (used during a fault, quorum check).
# assert_peer_count_ge <peers> <min> <label>
assert_peer_count_ge() {
  local peers="$1" min="$2" label="${3:-peers}"
  if ! is_uint "$peers"; then
    record_fail "peers_readable[$label]" "non-numeric peer count ($peers)"
    return 1
  fi
  if [ "$peers" -lt "$min" ]; then
    record_fail "peer_count_ge[$label]" "peer_count $peers < min $min"
    return 1
  fi
  record_pass "peer_count_ge[$label]" "peer_count $peers >= $min"
  return 0
}

# --- Report ---------------------------------------------------------------

# _assert_render <use_color 0|1>: print the report body.
_assert_render() {
  local uc="$1" R='' G='' B='' NC=''
  if [ "$uc" -eq 1 ]; then R="$C_RED"; G="$C_GREEN"; B="$C_BOLD"; NC="$C_NC"; fi
  local total=$(( ASSERT_PASS + ASSERT_FAIL ))
  printf '\n%s===== STRESS ASSERTION REPORT =====%s\n' "$B" "$NC"
  printf 'timestamp:  %s\n' "$(stress_ts)"
  printf 'assertions: %s   pass: %s   fail: %s\n' "$total" "$ASSERT_PASS" "$ASSERT_FAIL"
  printf '%s\n' "-----------------------------------"
  local line status rest name detail
  for line in "${ASSERT_RESULTS[@]:-}"; do
    [ -z "$line" ] && continue
    status="${line%%|*}"
    rest="${line#*|}"
    name="${rest%%|*}"
    detail="${rest#*|}"
    printf '  %-4s  %-30s  %s\n' "$status" "$name" "$detail"
  done
  printf '%s\n' "-----------------------------------"
  if [ "$ASSERT_FAIL" -eq 0 ]; then
    printf '%sVERDICT: PASS%s\n' "$G" "$NC"
  else
    printf '%sVERDICT: FAIL (%s failed)%s\n' "$R" "$ASSERT_FAIL" "$NC"
  fi
}

# assert_report [report_file]
#   Prints a colored summary to stdout, appends a plain copy to report_file if
#   given, and returns non-zero if any assertion failed.
assert_report() {
  local file="${1:-}"
  _assert_render 1
  if [ -n "$file" ]; then
    _assert_render 0 >> "$file"
  fi
  [ "$ASSERT_FAIL" -eq 0 ]
}
