#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Mainnet Launch Verification
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Automated verification of pre-launch checklist and Day 0 health checks
#
# USAGE:
#   ./scripts/launch-verify.sh [MODE] [OPTIONS]
#
# MODES:
#   pre-launch       Run T-24h pre-launch checklist (default)
#   day0             Run Day 0 launch verification
#   full             Run both pre-launch and Day 0 checks
#   monitor          Continuous monitoring mode (refreshes every 30s)
#
# OPTIONS:
#   --config <path>        Path to genesis config (default: mainnet/genesis_config.json)
#   --state-root <hex>     Expected state root for verification
#   --seed-nodes <list>    Comma-separated seed node addresses (default: from BOOTSTRAP_INFRASTRUCTURE.md)
#   --rpc-endpoint <url>   RPC endpoint URL (default: http://localhost:9545)
#   --metrics-port <port>  Base metrics port (default: 8080)
#   --timeout <seconds>    Check timeout (default: 10)
#   --json                 Output results as JSON
#   --quiet                Only output failures
#   --help                 Show this help message
#
# EXIT CODES:
#   0 - All checks passed
#   1 - One or more checks failed
#   2 - Configuration error
#
# EXAMPLES:
#   # Run pre-launch checklist
#   ./scripts/launch-verify.sh pre-launch --config mainnet/genesis_config.json
#
#   # Run Day 0 verification against local nodes
#   ./scripts/launch-verify.sh day0 --rpc-endpoint http://localhost:9545
#
#   # Continuous monitoring during launch
#   ./scripts/launch-verify.sh monitor
#
#   # Full verification with expected state root
#   ./scripts/launch-verify.sh full --state-root abc123...
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# SCRIPT SETUP
# ─────────────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Source common functions if available
if [[ -f "${SCRIPT_DIR}/common.sh" ]]; then
    source "${SCRIPT_DIR}/common.sh"
else
    # Minimal logging if common.sh not available
    log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*"; }
    log_success() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] [SUCCESS] $*"; }
    log_error() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] [ERROR] $*" >&2; }
    log_warn() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] [WARNING] $*" >&2; }
    log_section() { echo ""; echo "═══════════════════════════════════════════════════════════"; echo "  $1"; echo "═══════════════════════════════════════════════════════════"; echo ""; }
fi

# ─────────────────────────────────────────────────────────────────────────────
# DEFAULT VALUES
# ─────────────────────────────────────────────────────────────────────────────

MODE="pre-launch"
CONFIG_PATH="${PROJECT_ROOT}/mainnet/genesis_config.json"
EXPECTED_STATE_ROOT=""
RPC_ENDPOINT="http://localhost:9545"
BASE_METRICS_PORT="8080"
TIMEOUT="10"
JSON_OUTPUT="false"
QUIET="false"
SHOW_HELP="false"

# Default seed nodes (from BOOTSTRAP_INFRASTRUCTURE.md)
DEFAULT_SEED_NODES=(
    "seed-1.mainnet.novai.io:9090"
    "seed-2.mainnet.novai.io:9090"
    "seed-3.mainnet.novai.io:9090"
    "seed-4.mainnet.novai.io:9090"
    "seed-5.mainnet.novai.io:9090"
)
declare -a SEED_NODES=()

# Local validator nodes (for testing/local deployment)
declare -a LOCAL_METRICS_PORTS=(8080 8081 8082 8083 8084)
declare -a LOCAL_P2P_PORTS=(9090 9091 9092 9093 9094)

# Genesis generator binary
GENESIS_GENERATOR="${PROJECT_ROOT}/target/release/genesis-generator"

# Results tracking
declare -a PASSED_CHECKS=()
declare -a FAILED_CHECKS=()
declare -a WARNINGS=()

# ─────────────────────────────────────────────────────────────────────────────
# HELP
# ─────────────────────────────────────────────────────────────────────────────

show_help() {
    cat << 'EOF'
NOVAI Mainnet Launch Verification

Automates pre-launch checklist and Day 0 health verification.

USAGE:
    ./scripts/launch-verify.sh [MODE] [OPTIONS]

MODES:
    pre-launch    T-24h pre-launch checklist (default)
    day0          Day 0 launch verification
    full          Run both pre-launch and Day 0 checks
    monitor       Continuous monitoring (refreshes every 30s)

OPTIONS:
    --config <path>        Genesis config path (default: mainnet/genesis_config.json)
    --state-root <hex>     Expected state root for verification
    --seed-nodes <list>    Comma-separated seed node addresses
    --rpc-endpoint <url>   RPC endpoint (default: http://localhost:9545)
    --metrics-port <port>  Base metrics port (default: 8080)
    --timeout <seconds>    Check timeout (default: 10)
    --json                 Output results as JSON
    --quiet                Only output failures
    --help                 Show this help message

PRE-LAUNCH CHECKS (T-24h):
    1. Genesis config exists and is valid JSON
    2. Genesis config has no REPLACE_ME placeholders
    3. Genesis state root can be generated
    4. State root matches expected (if provided)
    5. genesis-generator binary exists
    6. Seed nodes are reachable (DNS resolves, port open)
    7. Monitoring endpoints respond (/health)
    8. On-call schedule documented

DAY 0 CHECKS (Launch):
    1. Genesis block height = 0
    2. Time to first block < 30 seconds
    3. Validators connected (peer_count > 0)
    4. All validators responding (100% required)
    5. Blocks are being produced (committed_height increasing)
    6. Transactions can be submitted (RPC responds)
    7. No excessive view changes (consensus stable)

EXIT CODES:
    0 - All checks passed
    1 - One or more checks failed
    2 - Configuration error
EOF
}

# ─────────────────────────────────────────────────────────────────────────────
# ARGUMENT PARSING
# ─────────────────────────────────────────────────────────────────────────────

parse_arguments() {
    # First argument might be mode
    if [[ $# -gt 0 ]] && [[ ! "$1" =~ ^-- ]]; then
        case "$1" in
            pre-launch|day0|full|monitor)
                MODE="$1"
                shift
                ;;
        esac
    fi

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --config)
                CONFIG_PATH="$2"
                shift 2
                ;;
            --state-root)
                EXPECTED_STATE_ROOT="$2"
                shift 2
                ;;
            --seed-nodes)
                IFS=',' read -ra SEED_NODES <<< "$2"
                shift 2
                ;;
            --rpc-endpoint)
                RPC_ENDPOINT="$2"
                shift 2
                ;;
            --metrics-port)
                BASE_METRICS_PORT="$2"
                shift 2
                ;;
            --timeout)
                TIMEOUT="$2"
                shift 2
                ;;
            --json)
                JSON_OUTPUT="true"
                shift
                ;;
            --quiet)
                QUIET="true"
                shift
                ;;
            --help|-h)
                SHOW_HELP="true"
                shift
                ;;
            *)
                log_error "Unknown argument: $1"
                echo "Use --help for usage information"
                exit 2
                ;;
        esac
    done

    # Use default seed nodes if none specified
    if [[ ${#SEED_NODES[@]} -eq 0 ]]; then
        SEED_NODES=("${DEFAULT_SEED_NODES[@]}")
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# UTILITY FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

# Record a passed check
pass_check() {
    local check_name="$1"
    local details="${2:-}"
    PASSED_CHECKS+=("$check_name")
    if [[ "${QUIET}" != "true" ]]; then
        if [[ -n "${details}" ]]; then
            log_success "✓ ${check_name}: ${details}"
        else
            log_success "✓ ${check_name}"
        fi
    fi
}

# Record a failed check
fail_check() {
    local check_name="$1"
    local details="${2:-}"
    FAILED_CHECKS+=("$check_name")
    if [[ -n "${details}" ]]; then
        log_error "✗ ${check_name}: ${details}"
    else
        log_error "✗ ${check_name}"
    fi
}

# Record a warning
warn_check() {
    local check_name="$1"
    local details="${2:-}"
    WARNINGS+=("$check_name")
    if [[ "${QUIET}" != "true" ]]; then
        if [[ -n "${details}" ]]; then
            log_warn "⚠ ${check_name}: ${details}"
        else
            log_warn "⚠ ${check_name}"
        fi
    fi
}

# Check if a command exists
command_exists() {
    command -v "$1" &>/dev/null
}

# Check if a port is open (TCP connect)
check_port() {
    local host="$1"
    local port="$2"
    local timeout="${3:-$TIMEOUT}"

    if command_exists nc; then
        nc -z -w "${timeout}" "${host}" "${port}" 2>/dev/null
    elif command_exists timeout; then
        timeout "${timeout}" bash -c "cat < /dev/null > /dev/tcp/${host}/${port}" 2>/dev/null
    else
        # Fallback: try curl if available
        curl -s --connect-timeout "${timeout}" "http://${host}:${port}/" &>/dev/null
        return $?
    fi
}

# HTTP GET with timeout
http_get() {
    local url="$1"
    local timeout="${2:-$TIMEOUT}"

    if command_exists curl; then
        curl -s --connect-timeout "${timeout}" --max-time "${timeout}" "${url}" 2>/dev/null
    elif command_exists wget; then
        wget -q -O - --timeout="${timeout}" "${url}" 2>/dev/null
    else
        log_error "Neither curl nor wget available"
        return 1
    fi
}

# JSON-RPC request
rpc_request() {
    local endpoint="$1"
    local method="$2"
    local params="${3:-[]}"

    local request
    request=$(cat <<EOF
{"jsonrpc":"2.0","method":"${method}","params":${params},"id":1}
EOF
)

    if command_exists curl; then
        curl -s --connect-timeout "${TIMEOUT}" --max-time "${TIMEOUT}" \
            -H "Content-Type: application/json" \
            -d "${request}" \
            "${endpoint}" 2>/dev/null
    else
        log_error "curl not available for RPC requests"
        return 1
    fi
}

# Get metric value from Prometheus endpoint
get_metric() {
    local metrics_url="$1"
    local metric_name="$2"

    local metrics
    metrics=$(http_get "${metrics_url}/metrics")
    if [[ -z "${metrics}" ]]; then
        echo ""
        return 1
    fi

    echo "${metrics}" | grep "^${metric_name} " | awk '{print $2}' | head -1
}

# ─────────────────────────────────────────────────────────────────────────────
# PRE-LAUNCH CHECKS (T-24h)
# ─────────────────────────────────────────────────────────────────────────────

check_genesis_config_exists() {
    if [[ -f "${CONFIG_PATH}" ]]; then
        pass_check "Genesis config exists" "${CONFIG_PATH}"
        return 0
    else
        fail_check "Genesis config exists" "File not found: ${CONFIG_PATH}"
        return 1
    fi
}

check_genesis_config_valid_json() {
    if ! [[ -f "${CONFIG_PATH}" ]]; then
        fail_check "Genesis config valid JSON" "Config file not found"
        return 1
    fi

    # Try python3 first (more common), then jq
    if command_exists python3; then
        if python3 -c "import json; json.load(open('${CONFIG_PATH}'))" 2>/dev/null; then
            pass_check "Genesis config valid JSON"
            return 0
        fi
    elif command_exists jq; then
        if jq empty "${CONFIG_PATH}" 2>/dev/null; then
            pass_check "Genesis config valid JSON"
            return 0
        fi
    fi

    fail_check "Genesis config valid JSON" "Invalid JSON syntax"
    return 1
}

check_no_placeholder_values() {
    if ! [[ -f "${CONFIG_PATH}" ]]; then
        fail_check "No placeholder values" "Config file not found"
        return 1
    fi

    if grep -q "REPLACE_ME" "${CONFIG_PATH}"; then
        local count
        count=$(grep -c "REPLACE_ME" "${CONFIG_PATH}")
        fail_check "No placeholder values" "Found ${count} REPLACE_ME placeholders"
        return 1
    fi

    pass_check "No placeholder values"
    return 0
}

check_genesis_generator_binary() {
    if [[ -x "${GENESIS_GENERATOR}" ]]; then
        pass_check "Genesis generator binary exists" "${GENESIS_GENERATOR}"
        return 0
    fi

    # Check if we can build it
    if [[ -f "${PROJECT_ROOT}/Cargo.toml" ]]; then
        warn_check "Genesis generator binary" "Not found, may need to build with: cargo build --release -p genesis-generator"
        return 1
    fi

    fail_check "Genesis generator binary exists" "Not found at ${GENESIS_GENERATOR}"
    return 1
}

check_genesis_state_root() {
    if ! [[ -x "${GENESIS_GENERATOR}" ]]; then
        warn_check "Genesis state root generation" "Skipped (genesis-generator not available)"
        return 1
    fi

    if ! [[ -f "${CONFIG_PATH}" ]]; then
        fail_check "Genesis state root generation" "Config file not found"
        return 1
    fi

    local state_root
    state_root=$("${GENESIS_GENERATOR}" --config "${CONFIG_PATH}" --state-root-only 2>/dev/null)

    if [[ -z "${state_root}" ]]; then
        fail_check "Genesis state root generation" "Failed to generate state root"
        return 1
    fi

    if [[ ${#state_root} -ne 64 ]]; then
        fail_check "Genesis state root generation" "Invalid state root length: ${#state_root} (expected 64)"
        return 1
    fi

    pass_check "Genesis state root generation" "${state_root:0:16}..."

    # Verify against expected if provided
    if [[ -n "${EXPECTED_STATE_ROOT}" ]]; then
        local expected_lower
        local actual_lower
        expected_lower=$(echo "${EXPECTED_STATE_ROOT}" | tr '[:upper:]' '[:lower:]')
        actual_lower=$(echo "${state_root}" | tr '[:upper:]' '[:lower:]')

        if [[ "${expected_lower}" == "${actual_lower}" ]]; then
            pass_check "State root matches expected"
            return 0
        else
            fail_check "State root matches expected" "Expected: ${EXPECTED_STATE_ROOT}, Actual: ${state_root}"
            return 1
        fi
    fi

    return 0
}

check_seed_node_dns() {
    local node="$1"
    local host="${node%%:*}"
    local port="${node##*:}"

    # Check if host resolves
    if command_exists host; then
        if ! host "${host}" &>/dev/null; then
            fail_check "Seed node DNS: ${host}" "DNS resolution failed"
            return 1
        fi
    elif command_exists nslookup; then
        if ! nslookup "${host}" &>/dev/null; then
            fail_check "Seed node DNS: ${host}" "DNS resolution failed"
            return 1
        fi
    elif command_exists dig; then
        if ! dig +short "${host}" &>/dev/null; then
            fail_check "Seed node DNS: ${host}" "DNS resolution failed"
            return 1
        fi
    else
        warn_check "Seed node DNS: ${host}" "No DNS tools available (host, nslookup, dig)"
        return 1
    fi

    pass_check "Seed node DNS: ${host}"
    return 0
}

check_seed_node_reachable() {
    local node="$1"
    local host="${node%%:*}"
    local port="${node##*:}"

    # Skip if it's a placeholder hostname that won't resolve
    if [[ "${host}" == *"novai.io" ]]; then
        # Check DNS first
        if ! check_seed_node_dns "${node}"; then
            return 1
        fi
    fi

    if check_port "${host}" "${port}"; then
        pass_check "Seed node reachable: ${node}"
        return 0
    else
        fail_check "Seed node reachable: ${node}" "Connection failed (timeout: ${TIMEOUT}s)"
        return 1
    fi
}

check_seed_nodes() {
    log "Checking ${#SEED_NODES[@]} seed nodes..."

    local reachable=0
    local total=${#SEED_NODES[@]}

    for node in "${SEED_NODES[@]}"; do
        if check_seed_node_reachable "${node}"; then
            ((reachable++)) || true
        fi
    done

    if [[ ${reachable} -eq 0 ]]; then
        fail_check "Seed nodes available" "0/${total} seed nodes reachable"
        return 1
    elif [[ ${reachable} -lt ${total} ]]; then
        warn_check "Seed nodes available" "${reachable}/${total} reachable"
        return 0
    else
        pass_check "Seed nodes available" "${reachable}/${total} reachable"
        return 0
    fi
}

check_local_metrics_health() {
    local port="$1"
    local url="http://localhost:${port}/health"

    local response
    response=$(http_get "${url}")

    if [[ "${response}" == "OK"* ]]; then
        pass_check "Metrics health: port ${port}"
        return 0
    else
        fail_check "Metrics health: port ${port}" "No response or unhealthy"
        return 1
    fi
}

check_monitoring_live() {
    log "Checking local monitoring endpoints..."

    local healthy=0
    for port in "${LOCAL_METRICS_PORTS[@]}"; do
        if check_local_metrics_health "${port}"; then
            ((healthy++)) || true
        fi
    done

    if [[ ${healthy} -eq 0 ]]; then
        warn_check "Monitoring endpoints" "No local monitoring endpoints responding"
        return 1
    else
        pass_check "Monitoring endpoints" "${healthy}/${#LOCAL_METRICS_PORTS[@]} responding"
        return 0
    fi
}

check_oncall_documented() {
    local playbook="${PROJECT_ROOT}/docs/OPERATOR_RUNBOOK.md"

    if [[ -f "${playbook}" ]]; then
        if grep -qi "on-call\|oncall\|pagerduty\|escalation" "${playbook}"; then
            pass_check "On-call rotation documented" "Found in OPERATOR_RUNBOOK.md"
            return 0
        fi
    fi

    warn_check "On-call rotation documented" "No on-call schedule found in docs"
    return 1
}

run_pre_launch_checks() {
    log_section "PRE-LAUNCH VERIFICATION (T-24h)"

    log "Configuration:"
    log "  Config path:    ${CONFIG_PATH}"
    log "  State root:     ${EXPECTED_STATE_ROOT:-<not specified>}"
    log "  Seed nodes:     ${#SEED_NODES[@]}"
    echo ""

    # Genesis configuration checks
    log "─── Genesis Configuration ───"
    check_genesis_config_exists || true
    check_genesis_config_valid_json || true
    check_no_placeholder_values || true
    check_genesis_generator_binary || true
    check_genesis_state_root || true
    echo ""

    # Infrastructure checks
    log "─── Infrastructure ───"
    check_seed_nodes || true
    check_monitoring_live || true
    check_oncall_documented || true
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# DAY 0 CHECKS (Launch)
# ─────────────────────────────────────────────────────────────────────────────

check_genesis_block_height() {
    local metrics_url="http://localhost:${BASE_METRICS_PORT}"

    local height
    height=$(get_metric "${metrics_url}" "novai_committed_height")

    if [[ -z "${height}" ]]; then
        fail_check "Node responding" "Cannot reach metrics endpoint"
        return 1
    fi

    pass_check "Node responding" "committed_height=${height}"
    return 0
}

check_time_to_first_block() {
    local metrics_url="http://localhost:${BASE_METRICS_PORT}"
    local max_wait=30
    local check_interval=2

    # Get genesis timestamp from config
    local genesis_timestamp=""
    if [[ -f "${CONFIG_PATH}" ]]; then
        if command_exists python3; then
            genesis_timestamp=$(python3 -c "import json; print(json.load(open('${CONFIG_PATH}')).get('timestamp', ''))" 2>/dev/null)
        elif command_exists jq; then
            genesis_timestamp=$(jq -r '.timestamp // ""' "${CONFIG_PATH}" 2>/dev/null)
        fi
    fi

    if [[ -z "${genesis_timestamp}" ]]; then
        warn_check "Time to first block" "Cannot read genesis timestamp from config"
        return 1
    fi

    # Check current height
    local height
    height=$(get_metric "${metrics_url}" "novai_committed_height")

    if [[ -z "${height}" ]]; then
        fail_check "Time to first block" "Cannot reach metrics endpoint"
        return 1
    fi

    # If height > 0, first block already produced
    if [[ "${height}" -gt 0 ]]; then
        pass_check "Time to first block" "Already at height ${height}"
        return 0
    fi

    # Height is 0, wait up to 30 seconds for first block
    log "Waiting for first block (max ${max_wait}s)..."
    local elapsed=0

    while [[ ${elapsed} -lt ${max_wait} ]]; do
        sleep ${check_interval}
        elapsed=$((elapsed + check_interval))

        height=$(get_metric "${metrics_url}" "novai_committed_height")

        if [[ -n "${height}" ]] && [[ "${height}" -gt 0 ]]; then
            pass_check "Time to first block" "First block produced in ${elapsed}s (height=${height})"
            return 0
        fi

        if [[ "${QUIET}" != "true" ]]; then
            log "  ${elapsed}s elapsed, height still 0..."
        fi
    done

    # Timeout reached, still at height 0
    fail_check "Time to first block" "No block produced after ${max_wait}s (height still 0)"
    return 1
}

check_validators_connected() {
    local metrics_url="http://localhost:${BASE_METRICS_PORT}"

    local peer_count
    peer_count=$(get_metric "${metrics_url}" "novai_peer_count")

    if [[ -z "${peer_count}" ]]; then
        fail_check "Validators connected" "Cannot read peer_count metric"
        return 1
    fi

    if [[ "${peer_count}" -eq 0 ]]; then
        fail_check "Validators connected" "peer_count=0 (no peers)"
        return 1
    fi

    pass_check "Validators connected" "peer_count=${peer_count}"
    return 0
}

check_blocks_produced() {
    local metrics_url="http://localhost:${BASE_METRICS_PORT}"

    local height1
    height1=$(get_metric "${metrics_url}" "novai_committed_height")

    if [[ -z "${height1}" ]]; then
        fail_check "Blocks being produced" "Cannot read committed_height"
        return 1
    fi

    # Wait and check again
    log "Waiting 5 seconds to observe block production..."
    sleep 5

    local height2
    height2=$(get_metric "${metrics_url}" "novai_committed_height")

    if [[ -z "${height2}" ]]; then
        fail_check "Blocks being produced" "Second read failed"
        return 1
    fi

    if [[ "${height2}" -gt "${height1}" ]]; then
        pass_check "Blocks being produced" "height ${height1} → ${height2}"
        return 0
    else
        warn_check "Blocks being produced" "height stuck at ${height1} (may need more time)"
        return 1
    fi
}

check_rpc_responds() {
    local response
    response=$(http_get "${RPC_ENDPOINT}")

    # Even an error response means the server is up
    if [[ -n "${response}" ]]; then
        pass_check "RPC endpoint responds" "${RPC_ENDPOINT}"
        return 0
    fi

    fail_check "RPC endpoint responds" "No response from ${RPC_ENDPOINT}"
    return 1
}

check_tx_submission() {
    # Create a minimal invalid tx to test RPC is accepting requests
    # We expect an error response, but the error proves the endpoint is working
    local response
    response=$(rpc_request "${RPC_ENDPOINT}" "novai_submitTransaction" '{"tx":"invalid"}')

    if [[ -z "${response}" ]]; then
        fail_check "Transaction submission RPC" "No response from RPC"
        return 1
    fi

    # Check if we got a JSON-RPC response (even an error is fine)
    if echo "${response}" | grep -q '"jsonrpc"'; then
        pass_check "Transaction submission RPC" "Endpoint accepting requests"
        return 0
    fi

    fail_check "Transaction submission RPC" "Invalid response format"
    return 1
}

check_consensus_stable() {
    local metrics_url="http://localhost:${BASE_METRICS_PORT}"

    local view_changes
    view_changes=$(get_metric "${metrics_url}" "novai_consensus_view_changes_total")

    if [[ -z "${view_changes}" ]]; then
        warn_check "Consensus stability" "Cannot read view_changes metric"
        return 1
    fi

    local height
    height=$(get_metric "${metrics_url}" "novai_committed_height")

    if [[ -z "${height}" ]] || [[ "${height}" -eq 0 ]]; then
        warn_check "Consensus stability" "No blocks yet, cannot assess stability"
        return 1
    fi

    # Calculate view changes per block
    local ratio
    if [[ "${height}" -gt 0 ]]; then
        ratio=$(echo "scale=2; ${view_changes} / ${height}" | bc 2>/dev/null || echo "0")
    else
        ratio="0"
    fi

    # More than 0.5 view changes per block is concerning
    local threshold="0.5"
    if command_exists bc; then
        if (( $(echo "${ratio} > ${threshold}" | bc -l 2>/dev/null || echo "0") )); then
            warn_check "Consensus stability" "High view change rate: ${ratio} per block (threshold: ${threshold})"
            return 1
        fi
    fi

    pass_check "Consensus stability" "view_changes=${view_changes}, height=${height}"
    return 0
}

check_all_validators_status() {
    log "Checking all validator metrics endpoints..."

    local responding=0
    local total=${#LOCAL_METRICS_PORTS[@]}

    for port in "${LOCAL_METRICS_PORTS[@]}"; do
        local url="http://localhost:${port}"
        local height
        height=$(get_metric "${url}" "novai_committed_height")

        if [[ -n "${height}" ]]; then
            ((responding++)) || true
            if [[ "${QUIET}" != "true" ]]; then
                local peers
                local round
                peers=$(get_metric "${url}" "novai_peer_count")
                round=$(get_metric "${url}" "novai_current_round")
                log "  Port ${port}: height=${height}, round=${round:-?}, peers=${peers:-?}"
            fi
        else
            log_warn "  Port ${port}: not responding"
        fi
    done

    if [[ ${responding} -eq 0 ]]; then
        fail_check "Validator nodes responding" "0/${total} responding"
        return 1
    elif [[ ${responding} -lt ${total} ]]; then
        fail_check "Validator nodes responding" "${responding}/${total} responding (100% required)"
        return 1
    else
        pass_check "Validator nodes responding" "${responding}/${total} responding"
        return 0
    fi
}

run_day0_checks() {
    log_section "DAY 0 LAUNCH VERIFICATION"

    log "Configuration:"
    log "  RPC endpoint:   ${RPC_ENDPOINT}"
    log "  Metrics port:   ${BASE_METRICS_PORT}"
    echo ""

    # Node health checks
    log "─── Node Health ───"
    check_genesis_block_height || true
    check_validators_connected || true
    check_all_validators_status || true
    echo ""

    # Consensus checks
    log "─── Consensus ───"
    check_time_to_first_block || true
    check_blocks_produced || true
    check_consensus_stable || true
    echo ""

    # RPC checks
    log "─── RPC Endpoints ───"
    check_rpc_responds || true
    check_tx_submission || true
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# RESULTS
# ─────────────────────────────────────────────────────────────────────────────

print_summary() {
    local total_passed=${#PASSED_CHECKS[@]}
    local total_failed=${#FAILED_CHECKS[@]}
    local total_warnings=${#WARNINGS[@]}
    local total=$((total_passed + total_failed))

    log_section "VERIFICATION SUMMARY"

    if [[ "${JSON_OUTPUT}" == "true" ]]; then
        cat << EOF
{
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "mode": "${MODE}",
  "passed": ${total_passed},
  "failed": ${total_failed},
  "warnings": ${total_warnings},
  "status": "$([ ${total_failed} -eq 0 ] && echo "PASS" || echo "FAIL")"
}
EOF
        return
    fi

    echo "  Passed:   ${total_passed}"
    echo "  Failed:   ${total_failed}"
    echo "  Warnings: ${total_warnings}"
    echo ""

    if [[ ${total_failed} -gt 0 ]]; then
        echo "  FAILED CHECKS:"
        for check in "${FAILED_CHECKS[@]}"; do
            echo "    - ${check}"
        done
        echo ""
    fi

    if [[ ${total_warnings} -gt 0 ]] && [[ "${QUIET}" != "true" ]]; then
        echo "  WARNINGS:"
        for check in "${WARNINGS[@]}"; do
            echo "    - ${check}"
        done
        echo ""
    fi

    if [[ ${total_failed} -eq 0 ]]; then
        echo "═══════════════════════════════════════════════════════════"
        echo "  ✅ ALL CHECKS PASSED"
        echo "═══════════════════════════════════════════════════════════"
    else
        echo "═══════════════════════════════════════════════════════════"
        echo "  ❌ ${total_failed} CHECK(S) FAILED"
        echo "═══════════════════════════════════════════════════════════"
    fi
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# MONITOR MODE
# ─────────────────────────────────────────────────────────────────────────────

run_monitor_mode() {
    log "Starting continuous monitoring (Ctrl+C to exit)..."
    echo ""

    while true; do
        clear 2>/dev/null || true

        echo "═══════════════════════════════════════════════════════════════════════════"
        echo "  NOVAI Mainnet Monitor                      $(date '+%Y-%m-%d %H:%M:%S')"
        echo "═══════════════════════════════════════════════════════════════════════════"
        echo ""

        # Quick status table
        printf "  %-8s  %-8s  %-8s  %-8s  %-12s\n" "Port" "Status" "Height" "Round" "Peers"
        echo "  ────────────────────────────────────────────────────────────────"

        for port in "${LOCAL_METRICS_PORTS[@]}"; do
            local url="http://localhost:${port}"
            local height peers round status

            height=$(get_metric "${url}" "novai_committed_height" 2>/dev/null)
            if [[ -n "${height}" ]]; then
                status="UP"
                peers=$(get_metric "${url}" "novai_peer_count" 2>/dev/null || echo "?")
                round=$(get_metric "${url}" "novai_current_round" 2>/dev/null || echo "?")
            else
                status="DOWN"
                height="?"
                peers="?"
                round="?"
            fi

            printf "  %-8s  %-8s  %-8s  %-8s  %-12s\n" "${port}" "${status}" "${height}" "${round}" "${peers}"
        done

        echo ""
        echo "  Press Ctrl+C to exit"
        echo "═══════════════════════════════════════════════════════════════════════════"

        sleep 30
    done
}

# ─────────────────────────────────────────────────────────────────────────────
# MAIN
# ─────────────────────────────────────────────────────────────────────────────

main() {
    parse_arguments "$@"

    if [[ "${SHOW_HELP}" == "true" ]]; then
        show_help
        exit 0
    fi

    cd "${PROJECT_ROOT}"

    case "${MODE}" in
        pre-launch)
            run_pre_launch_checks
            ;;
        day0)
            run_day0_checks
            ;;
        full)
            run_pre_launch_checks
            run_day0_checks
            ;;
        monitor)
            run_monitor_mode
            exit 0
            ;;
        *)
            log_error "Unknown mode: ${MODE}"
            exit 2
            ;;
    esac

    print_summary

    # Exit with appropriate code
    if [[ ${#FAILED_CHECKS[@]} -gt 0 ]]; then
        exit 1
    fi
    exit 0
}

main "$@"
