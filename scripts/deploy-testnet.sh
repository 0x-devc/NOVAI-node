#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Deployment - Full Testnet (5 Validators)
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Deploy a complete 5-validator NOVAI testnet
#
# USAGE:
#   ./scripts/deploy-testnet.sh [OPTIONS]
#
# OPTIONS:
#   --environment, --env   Target environment: local|aws|digitalocean (default: local)
#   --clean                Remove all existing containers/volumes before starting
#   --dry-run              Print actions without executing
#   --force                Skip confirmation prompts
#   --debug                Enable debug logging
#   --help                 Show this help message
#
# NETWORK TOPOLOGY:
#   Validator 0 (seed):  172.28.0.10:9090 -> localhost:9090
#   Validator 1:         172.28.0.11:9090 -> localhost:9091  (peers to 0)
#   Validator 2:         172.28.0.12:9090 -> localhost:9092  (peers to 0)
#   Validator 3:         172.28.0.13:9090 -> localhost:9093  (peers to 0)
#   Validator 4:         172.28.0.14:9090 -> localhost:9094  (peers to 0)
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# SCRIPT SETUP
# ─────────────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

setup_error_trap

# ─────────────────────────────────────────────────────────────────────────────
# DEFAULT VALUES
# ─────────────────────────────────────────────────────────────────────────────

ENVIRONMENT="local"
CLEAN="false"
DRY_RUN="false"
FORCE="false"
SHOW_HELP="false"

readonly TESTNET_SIZE=5
readonly SEED_VALIDATOR=0

# ─────────────────────────────────────────────────────────────────────────────
# HELP
# ─────────────────────────────────────────────────────────────────────────────

show_help() {
    cat << 'EOF'
NOVAI Testnet Deployment Script

USAGE:
    ./scripts/deploy-testnet.sh [OPTIONS]

OPTIONS:
    --environment, --env   Target environment: local|aws|digitalocean
    --clean                Remove all existing containers/volumes before starting
    --dry-run              Print actions without executing
    --force                Skip confirmation prompts
    --debug                Enable debug logging
    --help                 Show this help message

EXAMPLES:
    ./scripts/deploy-testnet.sh
    ./scripts/deploy-testnet.sh --clean
    ./scripts/deploy-testnet.sh --dry-run

NETWORK TOPOLOGY:
    ┌─────────────┬────────────────┬──────────┬──────────┐
    │ Validator   │ Container IP   │ P2P Port │ Metrics  │
    ├─────────────┼────────────────┼──────────┼──────────┤
    │ validator-0 │ 172.28.0.10    │ 9090     │ 8080     │
    │ validator-1 │ 172.28.0.11    │ 9091     │ 8081     │
    │ validator-2 │ 172.28.0.12    │ 9092     │ 8082     │
    │ validator-3 │ 172.28.0.13    │ 9093     │ 8083     │
    │ validator-4 │ 172.28.0.14    │ 9094     │ 8084     │
    └─────────────┴────────────────┴──────────┴──────────┘

Validator 0 is the seed node. All others connect to it.
EOF
}

# ─────────────────────────────────────────────────────────────────────────────
# ARGUMENT PARSING
# ─────────────────────────────────────────────────────────────────────────────

parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --environment|--env)
                ENVIRONMENT="$2"
                shift 2
                ;;
            --clean)
                CLEAN="true"
                shift
                ;;
            --dry-run)
                DRY_RUN="true"
                shift
                ;;
            --force)
                FORCE="true"
                shift
                ;;
            --debug)
                NOVAI_DEBUG="1"
                export NOVAI_DEBUG
                shift
                ;;
            --help|-h)
                SHOW_HELP="true"
                shift
                ;;
            *)
                log_error "Unknown argument: $1"
                exit 1
                ;;
        esac
    done
}

# ─────────────────────────────────────────────────────────────────────────────
# VALIDATION
# ─────────────────────────────────────────────────────────────────────────────

validate_inputs() {
    case "${ENVIRONMENT}" in
        local|aws|digitalocean) ;;
        *)
            log_error "Invalid environment: ${ENVIRONMENT}"
            exit 1
            ;;
    esac

    if [[ "${ENVIRONMENT}" != "local" ]]; then
        log_warn "Cloud deployment detected: ${ENVIRONMENT}"
        log_warn "For cloud, consider deploying validators individually"
        if [[ "${FORCE}" != "true" ]]; then
            if ! confirm "Continue with cloud deployment?"; then
                exit 0
            fi
        fi
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# DRY RUN
# ─────────────────────────────────────────────────────────────────────────────

run_cmd() {
    if [[ "${DRY_RUN}" == "true" ]]; then
        log "[DRY-RUN] Would execute: $*"
        return 0
    fi
    "$@"
}

# ─────────────────────────────────────────────────────────────────────────────
# DEPLOYMENT FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

prepare_deployment() {
    log_section "Preparing Testnet Deployment"

    log "Configuration:"
    log "  Environment:    ${ENVIRONMENT}"
    log "  Testnet Size:   ${TESTNET_SIZE} validators"
    log "  Seed Node:      validator-${SEED_VALIDATOR}"

    if ! check_docker; then
        exit 1
    fi

    if ! check_image "${NOVAI_IMAGE}"; then
        exit 1
    fi

    if [[ ! -f "${PROJECT_ROOT}/testnet/genesis.json" ]]; then
        log_error "Genesis file not found: ${PROJECT_ROOT}/testnet/genesis.json"
        exit 1
    fi

    log_success "Prerequisites verified"
}

cleanup_testnet() {
    log_section "Cleaning Up Existing Testnet"

    for i in $(seq 0 $((TESTNET_SIZE - 1))); do
        local container
        container=$(get_container_name "${i}")

        if container_exists "${container}"; then
            log "Removing validator ${i}..."
            run_cmd cleanup_validator "${i}" "true"
        fi
    done

    if docker network inspect "${NOVAI_NETWORK}" &>/dev/null; then
        local containers
        containers=$(docker network inspect "${NOVAI_NETWORK}" --format '{{len .Containers}}' 2>/dev/null || echo "0")

        if [[ "${containers}" == "0" ]]; then
            run_cmd remove_network "${NOVAI_NETWORK}"
        fi
    fi

    log_success "Cleanup complete"
}

check_existing() {
    local existing=0

    for i in $(seq 0 $((TESTNET_SIZE - 1))); do
        local container
        container=$(get_container_name "${i}")

        if container_exists "${container}"; then
            existing=$((existing + 1))
        fi
    done

    if [[ "${existing}" -gt 0 ]]; then
        log_warn "Found ${existing} existing validator containers"

        if [[ "${CLEAN}" == "true" ]]; then
            cleanup_testnet
        else
            log "Use --clean to remove existing deployment"
            if [[ "${FORCE}" != "true" ]]; then
                if ! confirm "Continue and skip existing validators?"; then
                    exit 0
                fi
            fi
        fi
    fi
}

check_ports() {
    log "Checking port availability..."

    local conflicts=0

    for i in $(seq 0 $((TESTNET_SIZE - 1))); do
        local p2p_port
        local metrics_port
        p2p_port=$(get_p2p_port "${i}")
        metrics_port=$(get_metrics_port "${i}")

        local container
        container=$(get_container_name "${i}")
        if container_exists "${container}"; then
            continue
        fi

        if ! port_is_available "${p2p_port}"; then
            log_error "Validator ${i}: P2P port ${p2p_port} is in use"
            conflicts=$((conflicts + 1))
        fi

        if ! port_is_available "${metrics_port}"; then
            log_error "Validator ${i}: Metrics port ${metrics_port} is in use"
            conflicts=$((conflicts + 1))
        fi
    done

    if [[ "${conflicts}" -gt 0 ]]; then
        log_error "Found ${conflicts} port conflicts"
        exit 1
    fi

    log_success "All ports available"
}

get_seed_peer_address() {
    case "${ENVIRONMENT}" in
        local)
            echo "172.28.0.10:9090"
            ;;
        aws|digitalocean)
            local seed_ip
            seed_ip=$(get_host_ip "${ENVIRONMENT}")
            local seed_port
            seed_port=$(get_p2p_port "${SEED_VALIDATOR}")
            echo "${seed_ip}:${seed_port}"
            ;;
    esac
}

deploy_validator_node() {
    local validator_id="$1"
    local container
    local volume
    local p2p_port
    local metrics_port

    container=$(get_container_name "${validator_id}")
    volume=$(get_volume_name "${validator_id}")
    p2p_port=$(get_p2p_port "${validator_id}")
    metrics_port=$(get_metrics_port "${validator_id}")

    if container_is_running "${container}"; then
        log "Validator ${validator_id}: Already running (skipping)"
        return 0
    fi

    if container_exists "${container}"; then
        log "Validator ${validator_id}: Removing stopped container"
        run_cmd remove_container "${container}"
    fi

    log "Validator ${validator_id}: Deploying..."

    if ! docker volume inspect "${volume}" &>/dev/null; then
        run_cmd docker volume create "${volume}"
    fi

    local -a cmd=(
        docker run
        --detach
        --name "${container}"
        --hostname "${container}"
        --network "${NOVAI_NETWORK}"
        --ip "172.28.0.$((10 + validator_id))"
        --restart unless-stopped
        -p "${p2p_port}:9090"
        -p "${metrics_port}:8080"
        -v "${volume}:/data"
        -e "RUST_LOG=info"
        --label "novai.validator.id=${validator_id}"
        --label "novai.testnet=true"
        "${NOVAI_IMAGE}"
        run --port 9090 --validator "${validator_id}"
    )

    if [[ "${validator_id}" -ne "${SEED_VALIDATOR}" ]]; then
        local seed_peer
        seed_peer=$(get_seed_peer_address)
        cmd+=(--peer "${seed_peer}")
    fi

    if [[ "${DRY_RUN}" == "true" ]]; then
        log "[DRY-RUN] Would execute: ${cmd[*]}"
    else
        local container_id
        container_id=$("${cmd[@]}")
        log "Validator ${validator_id}: Started (${container_id:0:12})"

        if ! wait_for_container "${container}" "${p2p_port}" "127.0.0.1" 30; then
            log_error "Validator ${validator_id}: Failed to start"
            docker logs "${container}" --tail 20 2>&1 || true
            return 1
        fi

        log_success "Validator ${validator_id}: Ready"
    fi

    return 0
}

deploy_testnet() {
    log_section "Deploying Testnet"

    run_cmd create_network "${NOVAI_NETWORK}"

    log "Deploying seed node (validator ${SEED_VALIDATOR})..."
    if ! deploy_validator_node "${SEED_VALIDATOR}"; then
        log_error "Failed to deploy seed node"
        exit 1
    fi

    if [[ "${DRY_RUN}" != "true" ]]; then
        log "Waiting for seed node to initialize..."
        sleep 2
    fi

    for i in $(seq 0 $((TESTNET_SIZE - 1))); do
        if [[ "${i}" -eq "${SEED_VALIDATOR}" ]]; then
            continue
        fi

        if ! deploy_validator_node "${i}"; then
            log_error "Failed to deploy validator ${i}"
            log_warn "Continuing with remaining validators..."
        fi

        if [[ "${DRY_RUN}" != "true" ]]; then
            sleep 1
        fi
    done

    log_success "All validators deployed"
}

show_status() {
    log_section "Testnet Status"

    echo ""
    echo "Network: ${NOVAI_NETWORK}"
    echo "Genesis: testnet/genesis.json"
    echo ""

    printf "%-15s %-12s %-15s %-10s %-10s\n" "VALIDATOR" "STATUS" "CONTAINER IP" "P2P" "METRICS"
    printf "%-15s %-12s %-15s %-10s %-10s\n" "---------" "------" "------------" "---" "-------"

    local running=0
    local stopped=0

    for i in $(seq 0 $((TESTNET_SIZE - 1))); do
        local container
        local p2p_port
        local metrics_port
        local status
        local container_ip

        container=$(get_container_name "${i}")
        p2p_port=$(get_p2p_port "${i}")
        metrics_port=$(get_metrics_port "${i}")

        if container_is_running "${container}"; then
            status="${COLOR_GREEN}running${COLOR_RESET}"
            running=$((running + 1))
            container_ip=$(get_container_ip "${container}" "${NOVAI_NETWORK}" 2>/dev/null || echo "N/A")
        elif container_exists "${container}"; then
            status="${COLOR_YELLOW}stopped${COLOR_RESET}"
            stopped=$((stopped + 1))
            container_ip="N/A"
        else
            status="${COLOR_RED}missing${COLOR_RESET}"
            container_ip="N/A"
        fi

        local name="validator-${i}"
        if [[ "${i}" -eq "${SEED_VALIDATOR}" ]]; then
            name="${name} (seed)"
        fi

        printf "%-15s %-20s %-15s %-10s %-10s\n" \
            "${name}" "${status}" "${container_ip}" "${p2p_port}" "${metrics_port}"
    done

    echo ""
    echo "Summary: ${running} running, ${stopped} stopped"
    echo ""

    if [[ "${running}" -eq "${TESTNET_SIZE}" ]]; then
        log_success "All validators are running!"
        echo ""
        echo "Useful commands:"
        echo "  View logs:        docker logs -f novai-validator-0"
        echo "  Stop testnet:     ./scripts/cleanup.sh"
    fi

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

    validate_inputs

    cd "${PROJECT_ROOT}"

    prepare_deployment
    check_existing

    if [[ "${DRY_RUN}" != "true" ]]; then
        check_ports
    fi

    deploy_testnet

    if [[ "${DRY_RUN}" != "true" ]]; then
        show_status
    fi

    log_success "Testnet deployment complete!"
}

main "$@"
