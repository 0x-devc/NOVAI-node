#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Deployment - Single Validator Node
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Deploy a single NOVAI validator node as a Docker container
#
# USAGE:
#   ./scripts/deploy-validator.sh --validator-id <0-4> [OPTIONS]
#
# OPTIONS:
#   --validator-id, --validator  Validator index (0-4, required)
#   --environment, --env         Target environment: local|aws|digitalocean (default: local)
#   --port                       Override P2P port (default: 9090+validator_id)
#   --peer                       Peer address to connect to (can be repeated)
#   --clean                      Remove existing container/volume before starting
#   --dry-run                    Print actions without executing
#   --force                      Skip confirmation prompts
#   --debug                      Enable debug logging
#   --help                       Show this help message
#
# EXAMPLES:
#   # Deploy validator 0 (seed node, no peers)
#   ./scripts/deploy-validator.sh --validator-id 0
#
#   # Deploy validator 1, connecting to validator 0
#   ./scripts/deploy-validator.sh --validator-id 1 --peer 172.28.0.10:9090
#
#   # Clean deploy on AWS
#   ./scripts/deploy-validator.sh --validator-id 2 --env aws --clean
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# SCRIPT SETUP
# ─────────────────────────────────────────────────────────────────────────────

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Source common functions
# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

# Setup error handling
setup_error_trap

# ─────────────────────────────────────────────────────────────────────────────
# DEFAULT VALUES
# ─────────────────────────────────────────────────────────────────────────────

VALIDATOR_ID=""
ENVIRONMENT="local"
PORT=""
declare -a PEERS=()
CLEAN="false"
DRY_RUN="false"
FORCE="false"
SHOW_HELP="false"

# ─────────────────────────────────────────────────────────────────────────────
# HELP
# ─────────────────────────────────────────────────────────────────────────────

show_help() {
    cat << 'EOF'
NOVAI Validator Deployment Script

USAGE:
    ./scripts/deploy-validator.sh --validator-id <0-4> [OPTIONS]

OPTIONS:
    --validator-id, --validator  Validator index (0-4, required)
    --environment, --env         Target environment: local|aws|digitalocean
    --port                       Override P2P port (default: 9090+validator_id)
    --peer                       Peer address to connect to (can be repeated)
    --clean                      Remove existing container/volume before starting
    --dry-run                    Print actions without executing
    --force                      Skip confirmation prompts
    --debug                      Enable debug logging
    --help                       Show this help message

EXAMPLES:
    ./scripts/deploy-validator.sh --validator-id 0
    ./scripts/deploy-validator.sh --validator-id 1 --peer 172.28.0.10:9090
    ./scripts/deploy-validator.sh --validator-id 2 --env aws --clean
EOF
}

# ─────────────────────────────────────────────────────────────────────────────
# ARGUMENT PARSING
# ─────────────────────────────────────────────────────────────────────────────

parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --validator-id|--validator)
                VALIDATOR_ID="$2"
                shift 2
                ;;
            --environment|--env)
                ENVIRONMENT="$2"
                shift 2
                ;;
            --port)
                PORT="$2"
                shift 2
                ;;
            --peer)
                PEERS+=("$2")
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
                echo "Use --help for usage information"
                exit 1
                ;;
        esac
    done
}

# ─────────────────────────────────────────────────────────────────────────────
# VALIDATION
# ─────────────────────────────────────────────────────────────────────────────

validate_inputs() {
    if [[ -z "${VALIDATOR_ID}" ]]; then
        log_error "Missing required argument: --validator-id"
        exit 1
    fi

    if ! validate_validator_id "${VALIDATOR_ID}"; then
        exit 1
    fi

    case "${ENVIRONMENT}" in
        local|aws|digitalocean) ;;
        *)
            log_error "Invalid environment: ${ENVIRONMENT}"
            exit 1
            ;;
    esac

    if [[ -z "${PORT}" ]]; then
        PORT=$(get_p2p_port "${VALIDATOR_ID}")
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# DRY RUN WRAPPER
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
    log_section "Preparing Deployment for Validator ${VALIDATOR_ID}"

    log "Configuration:"
    log "  Validator ID:  ${VALIDATOR_ID}"
    log "  Environment:   ${ENVIRONMENT}"
    log "  P2P Port:      ${PORT}"
    log "  Metrics Port:  $(get_metrics_port "${VALIDATOR_ID}")"
    log "  Peers:         ${PEERS[*]:-none}"

    if ! check_docker; then
        exit 1
    fi

    if ! check_image "${NOVAI_IMAGE}"; then
        exit 1
    fi
}

handle_existing() {
    local container
    container=$(get_container_name "${VALIDATOR_ID}")

    if container_exists "${container}"; then
        if container_is_running "${container}"; then
            if [[ "${CLEAN}" == "true" ]]; then
                log_warn "Container ${container} is running, will be replaced"
                run_cmd cleanup_validator "${VALIDATOR_ID}" "true"
            else
                log_success "Container ${container} is already running"
                show_status
                exit 0
            fi
        else
            log_warn "Container ${container} exists but is not running"
            if [[ "${CLEAN}" == "true" ]]; then
                run_cmd cleanup_validator "${VALIDATOR_ID}" "true"
            else
                log "Starting existing container..."
                run_cmd docker start "${container}"
                local p2p_port
                p2p_port=$(get_p2p_port "${VALIDATOR_ID}")
                if wait_for_container "${container}" "${p2p_port}"; then
                    show_status
                    exit 0
                else
                    log_error "Container failed to start"
                    exit 1
                fi
            fi
        fi
    elif [[ "${CLEAN}" == "true" ]]; then
        local volume
        volume=$(get_volume_name "${VALIDATOR_ID}")
        run_cmd remove_volume "${volume}"
    fi
}

create_volume() {
    local volume
    volume=$(get_volume_name "${VALIDATOR_ID}")

    log "Creating data volume: ${volume}"

    if docker volume inspect "${volume}" &>/dev/null; then
        log_success "Volume ${volume} already exists"
        return 0
    fi

    run_cmd docker volume create "${volume}"
    log_success "Created volume: ${volume}"
}

build_docker_command() {
    local container
    local volume
    local p2p_port
    local metrics_port

    container=$(get_container_name "${VALIDATOR_ID}")
    volume=$(get_volume_name "${VALIDATOR_ID}")
    p2p_port=$(get_p2p_port "${VALIDATOR_ID}")
    metrics_port=$(get_metrics_port "${VALIDATOR_ID}")

    local -a cmd=(
        docker run
        --detach
        --name "${container}"
        --hostname "${container}"
        --restart unless-stopped
    )

    if [[ "${ENVIRONMENT}" == "local" ]]; then
        if docker network inspect "${NOVAI_NETWORK}" &>/dev/null; then
            cmd+=(--network "${NOVAI_NETWORK}")
            local ip="172.28.0.$((10 + VALIDATOR_ID))"
            cmd+=(--ip "${ip}")
        fi
    fi

    cmd+=(
        -p "${p2p_port}:9090"
        -p "${metrics_port}:8080"
        -v "${volume}:/data"
        -e "RUST_LOG=info"
        --label "novai.validator.id=${VALIDATOR_ID}"
        "${NOVAI_IMAGE}"
        run --port 9090 --validator "${VALIDATOR_ID}"
    )

    for peer in "${PEERS[@]}"; do
        cmd+=(--peer "${peer}")
    done

    echo "${cmd[@]}"
}

deploy_validator() {
    log_section "Deploying Validator ${VALIDATOR_ID}"

    if [[ "${ENVIRONMENT}" == "local" ]]; then
        run_cmd create_network "${NOVAI_NETWORK}"
    fi

    run_cmd create_volume

    if [[ "${DRY_RUN}" != "true" ]]; then
        local p2p_port
        local metrics_port
        p2p_port=$(get_p2p_port "${VALIDATOR_ID}")
        metrics_port=$(get_metrics_port "${VALIDATOR_ID}")

        if ! port_is_available "${p2p_port}"; then
            log_error "P2P port ${p2p_port} is already in use"
            exit 1
        fi

        if ! port_is_available "${metrics_port}"; then
            log_error "Metrics port ${metrics_port} is already in use"
            exit 1
        fi
    fi

    local docker_cmd
    docker_cmd=$(build_docker_command)

    log "Starting container..."

    if [[ "${DRY_RUN}" == "true" ]]; then
        log "[DRY-RUN] Would execute: ${docker_cmd}"
    else
        local container_id
        container_id=$(eval "${docker_cmd}")
        log_success "Container started: ${container_id:0:12}"

        local container
        local p2p_port
        container=$(get_container_name "${VALIDATOR_ID}")
        p2p_port=$(get_p2p_port "${VALIDATOR_ID}")

        if ! wait_for_container "${container}" "${p2p_port}"; then
            log_error "Container failed to become ready"
            docker logs "${container}" --tail 20 2>&1 || true
            exit 1
        fi
    fi
}

show_status() {
    local container
    local p2p_port
    local metrics_port

    container=$(get_container_name "${VALIDATOR_ID}")
    p2p_port=$(get_p2p_port "${VALIDATOR_ID}")
    metrics_port=$(get_metrics_port "${VALIDATOR_ID}")

    log_section "Validator ${VALIDATOR_ID} Status"

    echo ""
    echo "Container:      ${container}"
    echo "Status:         $(docker inspect "${container}" --format '{{.State.Status}}' 2>/dev/null || echo 'unknown')"
    echo "P2P Port:       ${p2p_port}"
    echo "Metrics Port:   ${metrics_port}"

    if [[ "${ENVIRONMENT}" == "local" ]]; then
        local container_ip
        container_ip=$(get_container_ip "${container}" "${NOVAI_NETWORK}" 2>/dev/null || echo "N/A")
        echo "Container IP:   ${container_ip}"
    fi

    echo ""
    echo "Commands:"
    echo "  Logs:         docker logs -f ${container}"
    echo "  Stop:         docker stop ${container}"
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
    handle_existing
    deploy_validator

    if [[ "${DRY_RUN}" != "true" ]]; then
        show_status
    fi

    log_success "Deployment complete!"
}

main "$@"
