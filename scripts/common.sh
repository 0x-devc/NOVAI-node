#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Deployment - Common Functions Library
# ═══════════════════════════════════════════════════════════════════════════════
# PURPOSE: Shared utility functions for all deployment scripts
#
# USAGE: source scripts/common.sh
#
# PROVIDES:
#   - Logging with timestamps and colors
#   - Docker environment verification
#   - Container and network management
#   - Validator key handling
#   - Port conflict detection
#   - Environment-specific configuration
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# CONSTANTS
# ─────────────────────────────────────────────────────────────────────────────

readonly NOVAI_IMAGE="${NOVAI_IMAGE:-novai-node:latest}"
readonly NOVAI_NETWORK="${NOVAI_NETWORK:-novai-testnet}"
readonly NOVAI_CONTAINER_PREFIX="${NOVAI_CONTAINER_PREFIX:-novai-validator}"

# Port ranges
readonly BASE_P2P_PORT="${BASE_P2P_PORT:-9090}"
readonly BASE_METRICS_PORT="${BASE_METRICS_PORT:-8080}"

# Timeouts
readonly CONTAINER_START_TIMEOUT="${CONTAINER_START_TIMEOUT:-30}"
readonly HEALTH_CHECK_TIMEOUT="${HEALTH_CHECK_TIMEOUT:-60}"

# Validator configuration (from genesis - Ed25519 public keys derived from dev seeds)
# Dev seeds: [i as u8; 32] for i in 0..5
# Public keys derived via: SigningKey::from_bytes(&[i; 32]).verifying_key()
readonly -a VALIDATOR_PUBKEYS=(
    "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29"
    "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c"
    "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394"
    "ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1"
    "ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c"
)

readonly NUM_VALIDATORS=${#VALIDATOR_PUBKEYS[@]}

# ─────────────────────────────────────────────────────────────────────────────
# COLOR CODES
# ─────────────────────────────────────────────────────────────────────────────

if [[ -t 1 ]] && command -v tput &>/dev/null; then
    readonly COLOR_RED=$(tput setaf 1)
    readonly COLOR_GREEN=$(tput setaf 2)
    readonly COLOR_YELLOW=$(tput setaf 3)
    readonly COLOR_BLUE=$(tput setaf 4)
    readonly COLOR_CYAN=$(tput setaf 6)
    readonly COLOR_RESET=$(tput sgr0)
    readonly COLOR_BOLD=$(tput bold)
else
    readonly COLOR_RED=""
    readonly COLOR_GREEN=""
    readonly COLOR_YELLOW=""
    readonly COLOR_BLUE=""
    readonly COLOR_CYAN=""
    readonly COLOR_RESET=""
    readonly COLOR_BOLD=""
fi

# ─────────────────────────────────────────────────────────────────────────────
# LOGGING FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

# Timestamp for log messages
_timestamp() {
    date "+%Y-%m-%d %H:%M:%S"
}

# Log informational message
log() {
    echo "${COLOR_CYAN}[$(_timestamp)]${COLOR_RESET} $*"
}

# Log success message
log_success() {
    echo "${COLOR_GREEN}[$(_timestamp)] [SUCCESS]${COLOR_RESET} $*"
}

# Log warning message
log_warn() {
    echo "${COLOR_YELLOW}[$(_timestamp)] [WARNING]${COLOR_RESET} $*" >&2
}

# Log error message
log_error() {
    echo "${COLOR_RED}[$(_timestamp)] [ERROR]${COLOR_RESET} $*" >&2
}

# Log debug message (only if NOVAI_DEBUG is set)
log_debug() {
    if [[ "${NOVAI_DEBUG:-}" == "1" ]]; then
        echo "${COLOR_BLUE}[$(_timestamp)] [DEBUG]${COLOR_RESET} $*"
    fi
}

# Log a section header
log_section() {
    local msg="$1"
    echo ""
    echo "${COLOR_BOLD}═══════════════════════════════════════════════════════════${COLOR_RESET}"
    echo "${COLOR_BOLD}  ${msg}${COLOR_RESET}"
    echo "${COLOR_BOLD}═══════════════════════════════════════════════════════════${COLOR_RESET}"
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# PREREQUISITE CHECKS
# ─────────────────────────────────────────────────────────────────────────────

# Check if Docker is available and running
check_docker() {
    log "Checking Docker availability..."

    if ! command -v docker &>/dev/null; then
        log_error "Docker is not installed. Please install Docker first."
        log_error "  - macOS: https://docs.docker.com/desktop/install/mac-install/"
        log_error "  - Linux: https://docs.docker.com/engine/install/"
        return 1
    fi

    if ! docker info &>/dev/null; then
        log_error "Docker daemon is not running. Please start Docker."
        log_error "  - macOS: Open Docker Desktop application"
        log_error "  - Linux: sudo systemctl start docker"
        return 1
    fi

    local docker_version
    docker_version=$(docker version --format '{{.Server.Version}}' 2>/dev/null || echo "unknown")
    log_success "Docker is available (version: ${docker_version})"
    return 0
}

# Check if the NOVAI Docker image exists, build if necessary
check_image() {
    local image="${1:-$NOVAI_IMAGE}"
    local build_if_missing="${2:-true}"

    log "Checking for Docker image: ${image}"

    if docker image inspect "${image}" &>/dev/null; then
        local image_id
        image_id=$(docker image inspect "${image}" --format '{{.Id}}' | cut -c 8-19)
        log_success "Image exists (ID: ${image_id})"
        return 0
    fi

    if [[ "${build_if_missing}" == "true" ]]; then
        log_warn "Image not found. Building from Dockerfile..."
        build_image "${image}"
        return $?
    else
        log_error "Image '${image}' not found and build_if_missing=false"
        return 1
    fi
}

# Build the Docker image
build_image() {
    local image="${1:-$NOVAI_IMAGE}"
    local dockerfile="${2:-Dockerfile}"
    local context="${3:-.}"

    log "Building Docker image: ${image}"

    if [[ ! -f "${dockerfile}" ]]; then
        log_error "Dockerfile not found at: ${dockerfile}"
        return 1
    fi

    local version
    local git_commit
    local build_date

    version=$(git describe --tags --always 2>/dev/null || echo "dev")
    git_commit=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
    build_date=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    log "  Version: ${version}"
    log "  Git commit: ${git_commit}"
    log "  Build date: ${build_date}"

    if ! docker build \
        --build-arg "VERSION=${version}" \
        --build-arg "GIT_COMMIT=${git_commit}" \
        --build-arg "BUILD_DATE=${build_date}" \
        -t "${image}" \
        -f "${dockerfile}" \
        "${context}"; then
        log_error "Failed to build Docker image"
        return 1
    fi

    log_success "Successfully built image: ${image}"
    return 0
}

# ─────────────────────────────────────────────────────────────────────────────
# NETWORK MANAGEMENT
# ─────────────────────────────────────────────────────────────────────────────

# Create Docker network if it doesn't exist
create_network() {
    local network="${1:-$NOVAI_NETWORK}"
    local subnet="${2:-172.28.0.0/16}"

    log "Checking Docker network: ${network}"

    if docker network inspect "${network}" &>/dev/null; then
        log_success "Network '${network}' already exists"
        return 0
    fi

    log "Creating Docker network: ${network} (subnet: ${subnet})"

    if ! docker network create \
        --driver bridge \
        --subnet "${subnet}" \
        "${network}"; then
        log_error "Failed to create Docker network: ${network}"
        return 1
    fi

    log_success "Created Docker network: ${network}"
    return 0
}

# Remove Docker network
remove_network() {
    local network="${1:-$NOVAI_NETWORK}"

    log "Removing Docker network: ${network}"

    if ! docker network inspect "${network}" &>/dev/null; then
        log_warn "Network '${network}' does not exist"
        return 0
    fi

    if ! docker network rm "${network}"; then
        log_error "Failed to remove network: ${network}"
        return 1
    fi

    log_success "Removed Docker network: ${network}"
    return 0
}

# Get container IP on the network
get_container_ip() {
    local container="$1"
    local network="${2:-$NOVAI_NETWORK}"

    docker inspect "${container}" \
        --format "{{.NetworkSettings.Networks.${network}.IPAddress}}" 2>/dev/null
}

# ─────────────────────────────────────────────────────────────────────────────
# CONTAINER MANAGEMENT
# ─────────────────────────────────────────────────────────────────────────────

# Get container name for validator
get_container_name() {
    local validator_id="$1"
    echo "${NOVAI_CONTAINER_PREFIX}-${validator_id}"
}

# Get volume name for validator
get_volume_name() {
    local validator_id="$1"
    echo "${NOVAI_CONTAINER_PREFIX}-${validator_id}-data"
}

# Check if container exists
container_exists() {
    local container="$1"
    docker container inspect "${container}" &>/dev/null
}

# Check if container is running
container_is_running() {
    local container="$1"
    local status
    status=$(docker container inspect "${container}" --format '{{.State.Running}}' 2>/dev/null || echo "false")
    [[ "${status}" == "true" ]]
}

# Stop container if running
stop_container() {
    local container="$1"
    local timeout="${2:-10}"

    if container_is_running "${container}"; then
        log "Stopping container: ${container}"
        if ! docker stop -t "${timeout}" "${container}"; then
            log_warn "Failed to gracefully stop container, forcing..."
            docker kill "${container}" || true
        fi
        log_success "Container stopped: ${container}"
    else
        log_debug "Container not running: ${container}"
    fi
}

# Remove container if exists
remove_container() {
    local container="$1"

    if container_exists "${container}"; then
        stop_container "${container}"
        log "Removing container: ${container}"
        docker rm -f "${container}" || true
        log_success "Container removed: ${container}"
    else
        log_debug "Container does not exist: ${container}"
    fi
}

# Remove volume if exists
remove_volume() {
    local volume="$1"

    if docker volume inspect "${volume}" &>/dev/null; then
        log "Removing volume: ${volume}"
        docker volume rm "${volume}" || true
        log_success "Volume removed: ${volume}"
    else
        log_debug "Volume does not exist: ${volume}"
    fi
}

# Full cleanup of a validator (container + volume)
cleanup_validator() {
    local validator_id="$1"
    local remove_data="${2:-false}"

    local container
    local volume
    container=$(get_container_name "${validator_id}")
    volume=$(get_volume_name "${validator_id}")

    log "Cleaning up validator ${validator_id}..."

    remove_container "${container}"

    if [[ "${remove_data}" == "true" ]]; then
        remove_volume "${volume}"
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# PORT MANAGEMENT
# ─────────────────────────────────────────────────────────────────────────────

# Calculate P2P port for validator
get_p2p_port() {
    local validator_id="$1"
    echo $((BASE_P2P_PORT + validator_id))
}

# Calculate metrics port for validator
get_metrics_port() {
    local validator_id="$1"
    echo $((BASE_METRICS_PORT + validator_id))
}

# Check if a port is available
port_is_available() {
    local port="$1"
    local host="${2:-127.0.0.1}"

    # Try to connect with timeout - if successful, port is in use
    ! timeout 1 bash -c "cat < /dev/null > /dev/tcp/${host}/${port}" 2>/dev/null
}

# Check if all ports for a validator are available
check_validator_ports() {
    local validator_id="$1"
    local p2p_port
    local metrics_port

    p2p_port=$(get_p2p_port "${validator_id}")
    metrics_port=$(get_metrics_port "${validator_id}")

    local all_available=true

    if ! port_is_available "${p2p_port}"; then
        log_error "P2P port ${p2p_port} is already in use"
        all_available=false
    fi

    if ! port_is_available "${metrics_port}"; then
        log_error "Metrics port ${metrics_port} is already in use"
        all_available=false
    fi

    [[ "${all_available}" == "true" ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# VALIDATOR KEY HANDLING
# ─────────────────────────────────────────────────────────────────────────────

# Get validator pubkey (from genesis constants)
get_validator_pubkey() {
    local validator_id="$1"

    if [[ "${validator_id}" -ge "${NUM_VALIDATORS}" ]]; then
        log_error "Invalid validator ID: ${validator_id} (max: $((NUM_VALIDATORS - 1)))"
        return 1
    fi

    echo "${VALIDATOR_PUBKEYS[${validator_id}]}"
}

# Validate validator ID
validate_validator_id() {
    local validator_id="$1"

    if ! [[ "${validator_id}" =~ ^[0-9]+$ ]]; then
        log_error "Validator ID must be a number: ${validator_id}"
        return 1
    fi

    if [[ "${validator_id}" -ge "${NUM_VALIDATORS}" ]]; then
        log_error "Validator ID out of range: ${validator_id} (valid: 0-$((NUM_VALIDATORS - 1)))"
        return 1
    fi

    return 0
}

# ─────────────────────────────────────────────────────────────────────────────
# HEALTH CHECKS
# ─────────────────────────────────────────────────────────────────────────────

# Wait for container to be healthy (TCP port check)
wait_for_container() {
    local container="$1"
    local port="$2"
    local host="${3:-127.0.0.1}"
    local timeout="${4:-$HEALTH_CHECK_TIMEOUT}"

    log "Waiting for container ${container} to be ready (port ${port})..."

    local start_time
    start_time=$(date +%s)

    while true; do
        local elapsed
        elapsed=$(($(date +%s) - start_time))

        if [[ "${elapsed}" -ge "${timeout}" ]]; then
            log_error "Timeout waiting for container ${container} (${timeout}s)"
            return 1
        fi

        # First check if container is still running
        if ! container_is_running "${container}"; then
            log_error "Container ${container} stopped unexpectedly"
            docker logs "${container}" --tail 20 2>/dev/null || true
            return 1
        fi

        # Then check if port is responding
        if ! port_is_available "${port}" "${host}"; then
            log_success "Container ${container} is ready (port ${port} responding)"
            return 0
        fi

        log_debug "Waiting... (${elapsed}s elapsed)"
        sleep 1
    done
}

# ─────────────────────────────────────────────────────────────────────────────
# ENVIRONMENT DETECTION
# ─────────────────────────────────────────────────────────────────────────────

# Detect current environment
detect_environment() {
    # Check for cloud metadata services
    if curl -s -m 1 http://169.254.169.254/latest/meta-data/ &>/dev/null; then
        echo "aws"
        return
    fi

    if curl -s -m 1 http://169.254.169.254/metadata/v1/ &>/dev/null; then
        echo "digitalocean"
        return
    fi

    # Default to local
    echo "local"
}

# Get host IP for environment
get_host_ip() {
    local environment="${1:-local}"

    case "${environment}" in
        local)
            echo "127.0.0.1"
            ;;
        aws)
            curl -s http://169.254.169.254/latest/meta-data/local-ipv4 2>/dev/null || echo "127.0.0.1"
            ;;
        digitalocean)
            curl -s http://169.254.169.254/metadata/v1/interfaces/private/0/ipv4/address 2>/dev/null || echo "127.0.0.1"
            ;;
        *)
            echo "127.0.0.1"
            ;;
    esac
}

# ─────────────────────────────────────────────────────────────────────────────
# UTILITY FUNCTIONS
# ─────────────────────────────────────────────────────────────────────────────

# Confirm action with user
confirm() {
    local prompt="${1:-Continue?}"
    local default="${2:-n}"

    if [[ "${FORCE:-}" == "true" ]]; then
        return 0
    fi

    local yn
    if [[ "${default}" == "y" ]]; then
        read -r -p "${prompt} [Y/n] " yn
        yn=${yn:-y}
    else
        read -r -p "${prompt} [y/N] " yn
        yn=${yn:-n}
    fi

    case "${yn}" in
        [Yy]* ) return 0 ;;
        * ) return 1 ;;
    esac
}

# ─────────────────────────────────────────────────────────────────────────────
# TRAP AND ERROR HANDLING
# ─────────────────────────────────────────────────────────────────────────────

# Error handler
_error_handler() {
    local exit_code=$?
    local line_no=$1
    log_error "Script failed at line ${line_no} with exit code ${exit_code}"
    exit "${exit_code}"
}

# Setup error trapping
setup_error_trap() {
    trap '_error_handler ${LINENO}' ERR
}
