#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Mainnet Genesis Generator
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Generate and verify deterministic mainnet genesis state
#
# USAGE:
#   ./scripts/generate-mainnet-genesis.sh [OPTIONS]
#
# OPTIONS:
#   --config <path>      Path to genesis config JSON (default: mainnet/genesis_config.json)
#   --output-dir <path>  Output directory for genesis files (default: mainnet/genesis-out)
#   --verify <hex>       Verify against expected state root (64 hex chars)
#   --dry-run            Print actions without executing
#   --force              Overwrite existing output directory
#   --help               Show this help message
#
# OUTPUTS:
#   mainnet/genesis-out/
#   ├── genesis_config.json    # Canonical config (for verification)
#   ├── genesis_block.bin      # Binary-encoded genesis block
#   ├── state_root.hex         # State root hash (64 hex chars)
#   ├── validator_set.json     # Sorted validator addresses
#   └── genesis_summary.txt    # Human-readable summary
#
# VERIFICATION:
#   Multiple parties should independently run this script with the same config
#   and verify they get identical state roots before mainnet launch.
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

CONFIG_PATH="${PROJECT_ROOT}/mainnet/genesis_config.json"
OUTPUT_DIR="${PROJECT_ROOT}/mainnet/genesis-out"
VERIFY_ROOT=""
DRY_RUN="false"
FORCE="false"
SHOW_HELP="false"

# Genesis generator binary
GENESIS_GENERATOR="${PROJECT_ROOT}/target/release/genesis-generator"

# ─────────────────────────────────────────────────────────────────────────────
# HELP
# ─────────────────────────────────────────────────────────────────────────────

show_help() {
    cat << 'EOF'
NOVAI Mainnet Genesis Generator

Generates deterministic genesis state for mainnet launch. Multiple parties
must independently run this with the same config and verify identical state roots.

USAGE:
    ./scripts/generate-mainnet-genesis.sh [OPTIONS]

OPTIONS:
    --config <path>      Path to genesis config JSON (default: mainnet/genesis_config.json)
    --output-dir <path>  Output directory for genesis files (default: mainnet/genesis-out)
    --verify <hex>       Verify against expected state root (64 hex chars)
    --dry-run            Print actions without executing
    --force              Overwrite existing output directory
    --help               Show this help message

EXAMPLES:
    # Generate genesis from default config
    ./scripts/generate-mainnet-genesis.sh

    # Verify against expected state root
    ./scripts/generate-mainnet-genesis.sh --verify abc123...

    # Use custom config
    ./scripts/generate-mainnet-genesis.sh --config custom_genesis.json

OUTPUT FILES:
    genesis_config.json  - Canonical config (for verification)
    genesis_block.bin    - Binary-encoded genesis block
    state_root.hex       - State root hash (64 hex chars)
    validator_set.json   - Sorted validator addresses
    genesis_summary.txt  - Human-readable summary

VERIFICATION PROCESS:
    1. Each validator operator runs this script independently
    2. Compare state_root.hex values
    3. All must match before proceeding with launch
    4. Any mismatch indicates config or tool difference
EOF
}

# ─────────────────────────────────────────────────────────────────────────────
# ARGUMENT PARSING
# ─────────────────────────────────────────────────────────────────────────────

parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --config)
                CONFIG_PATH="$2"
                shift 2
                ;;
            --output-dir)
                OUTPUT_DIR="$2"
                shift 2
                ;;
            --verify)
                VERIFY_ROOT="$2"
                shift 2
                ;;
            --dry-run)
                DRY_RUN="true"
                shift
                ;;
            --force)
                FORCE="true"
                shift
                ;;
            --help|-h)
                SHOW_HELP="true"
                shift
                ;;
            *)
                log_error "Unknown argument: $1"
                echo "Use --help for usage information"
                exit 1
                ;;
        esac
    done
}

# ─────────────────────────────────────────────────────────────────────────────
# VALIDATION
# ─────────────────────────────────────────────────────────────────────────────

validate_config() {
    log "Validating configuration..."

    # Check config file exists
    if [[ ! -f "${CONFIG_PATH}" ]]; then
        log_error "Config file not found: ${CONFIG_PATH}"
        exit 1
    fi

    # Check for placeholder values that must be replaced
    if grep -q "REPLACE_ME" "${CONFIG_PATH}"; then
        log_error "Config contains REPLACE_ME placeholders!"
        log_error "Please update all validator pubkeys and addresses before generating genesis."
        log_error ""
        log_error "Placeholders found:"
        grep -n "REPLACE_ME" "${CONFIG_PATH}" | head -10
        exit 1
    fi

    # Validate JSON syntax
    if ! python3 -c "import json; json.load(open('${CONFIG_PATH}'))" 2>/dev/null; then
        if ! jq empty "${CONFIG_PATH}" 2>/dev/null; then
            log_error "Invalid JSON syntax in config file"
            exit 1
        fi
    fi

    # Check required fields
    local chain_id
    chain_id=$(python3 -c "import json; print(json.load(open('${CONFIG_PATH}')).get('chain_id', ''))" 2>/dev/null || echo "")
    if [[ -z "${chain_id}" ]]; then
        log_error "Config missing required field: chain_id"
        exit 1
    fi

    # Warn if using testnet chain_id
    if [[ "${chain_id}" == *"testnet"* ]]; then
        log_warn "Config uses testnet chain_id: ${chain_id}"
        log_warn "Are you sure this is correct for mainnet?"
        if [[ "${FORCE}" != "true" ]]; then
            read -r -p "Continue anyway? [y/N] " yn
            if [[ ! "${yn}" =~ ^[Yy]$ ]]; then
                exit 0
            fi
        fi
    fi

    log_success "Config validation passed"
}

validate_binary() {
    log "Checking genesis-generator binary..."

    if [[ ! -x "${GENESIS_GENERATOR}" ]]; then
        log_warn "Genesis generator not found at: ${GENESIS_GENERATOR}"
        log "Building genesis-generator..."

        if [[ "${DRY_RUN}" == "true" ]]; then
            log "[DRY-RUN] Would run: cargo build --release -p genesis-generator"
        else
            cd "${PROJECT_ROOT}"
            if ! cargo build --release -p genesis-generator; then
                log_error "Failed to build genesis-generator"
                exit 1
            fi
        fi
    fi

    if [[ "${DRY_RUN}" != "true" ]]; then
        log_success "Genesis generator ready: ${GENESIS_GENERATOR}"
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# GENESIS GENERATION
# ─────────────────────────────────────────────────────────────────────────────

prepare_output_dir() {
    log "Preparing output directory: ${OUTPUT_DIR}"

    if [[ -d "${OUTPUT_DIR}" ]]; then
        if [[ "${FORCE}" == "true" ]]; then
            log_warn "Removing existing output directory..."
            if [[ "${DRY_RUN}" != "true" ]]; then
                rm -rf "${OUTPUT_DIR}"
            fi
        else
            log_error "Output directory already exists: ${OUTPUT_DIR}"
            log_error "Use --force to overwrite"
            exit 1
        fi
    fi

    if [[ "${DRY_RUN}" != "true" ]]; then
        mkdir -p "${OUTPUT_DIR}"
    fi
}

generate_genesis() {
    log_section "Generating Mainnet Genesis State"

    log "Config:     ${CONFIG_PATH}"
    log "Output:     ${OUTPUT_DIR}"

    if [[ -n "${VERIFY_ROOT}" ]]; then
        log "Verify:     ${VERIFY_ROOT}"
    fi

    echo ""

    if [[ "${DRY_RUN}" == "true" ]]; then
        log "[DRY-RUN] Would run: ${GENESIS_GENERATOR} --config ${CONFIG_PATH} --output-dir ${OUTPUT_DIR}"
        if [[ -n "${VERIFY_ROOT}" ]]; then
            log "[DRY-RUN] Would also verify against: ${VERIFY_ROOT}"
        fi
        return 0
    fi

    # Run genesis generator
    local cmd=("${GENESIS_GENERATOR}" --config "${CONFIG_PATH}" --output-dir "${OUTPUT_DIR}")

    if ! "${cmd[@]}"; then
        log_error "Genesis generation failed!"
        exit 1
    fi

    log_success "Genesis files generated successfully"

    # Display state root prominently
    echo ""
    echo "═══════════════════════════════════════════════════════════════════════════════"
    echo "  MAINNET STATE ROOT"
    echo "═══════════════════════════════════════════════════════════════════════════════"
    echo ""
    cat "${OUTPUT_DIR}/state_root.hex"
    echo ""
    echo "═══════════════════════════════════════════════════════════════════════════════"
    echo ""
}

verify_state_root() {
    if [[ -z "${VERIFY_ROOT}" ]]; then
        return 0
    fi

    log_section "Verifying State Root"

    local actual_root
    actual_root=$(cat "${OUTPUT_DIR}/state_root.hex")

    # Normalize to lowercase
    local expected_lower
    local actual_lower
    expected_lower=$(echo "${VERIFY_ROOT}" | tr '[:upper:]' '[:lower:]')
    actual_lower=$(echo "${actual_root}" | tr '[:upper:]' '[:lower:]')

    if [[ "${expected_lower}" == "${actual_lower}" ]]; then
        log_success "✅ VERIFICATION PASSED"
        log_success "State root matches expected value"
        return 0
    else
        log_error "❌ VERIFICATION FAILED"
        log_error ""
        log_error "Expected: ${VERIFY_ROOT}"
        log_error "Actual:   ${actual_root}"
        log_error ""
        log_error "State roots do not match!"
        log_error "This could indicate:"
        log_error "  - Different genesis config"
        log_error "  - Different genesis-generator version"
        log_error "  - Non-determinism bug (critical!)"
        exit 1
    fi
}

show_summary() {
    log_section "Genesis Generation Complete"

    echo "Output files:"
    ls -la "${OUTPUT_DIR}/"

    echo ""
    echo "Next steps:"
    echo "  1. Share state_root.hex with other validators"
    echo "  2. Each validator independently generates and verifies"
    echo "  3. All state roots must match before launch"
    echo "  4. Copy genesis files to validator nodes"
    echo ""
    echo "To verify another party's state root:"
    echo "  ./scripts/generate-mainnet-genesis.sh --verify <their_state_root_hex>"
    echo ""
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

    validate_config
    validate_binary
    prepare_output_dir
    generate_genesis

    if [[ "${DRY_RUN}" != "true" ]]; then
        verify_state_root
        show_summary
    fi

    log_success "Done!"
}

main "$@"
