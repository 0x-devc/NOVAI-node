# ═══════════════════════════════════════════════════════════════════════════════
# NOVAI Blockchain Node - Production Dockerfile
# ═══════════════════════════════════════════════════════════════════════════════
# Multi-stage build optimized for:
#   - Reproducibility (pinned Rust 1.84.0, locked dependencies)
#   - Build speed (cargo-chef for dependency caching)
#   - Minimal size (< 50MB using distroless-static)
#   - Security (non-root, minimal attack surface)
# ═══════════════════════════════════════════════════════════════════════════════

# -----------------------------------------------------------------------------
# Stage 0: Chef planner - Analyzes dependencies for caching
# -----------------------------------------------------------------------------
# cargo-chef extracts dependency information to create a "recipe" that can be
# cached separately from source code changes. This dramatically speeds up
# rebuilds when only application code changes (not dependencies).
FROM rust:1.84.0-bookworm AS chef

# Install cargo-chef for dependency caching optimization
# Version pinned for reproducibility
RUN cargo install cargo-chef --version 0.1.68 --locked

WORKDIR /build

# -----------------------------------------------------------------------------
# Stage 1: Recipe creation - Extracts dependency graph
# -----------------------------------------------------------------------------
FROM chef AS planner

# Copy only files needed for dependency analysis (Cargo.toml and Cargo.lock)
# This layer is invalidated only when dependencies change
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Generate the "recipe" - a manifest of all dependencies
# This step is fast and creates a cacheable layer
RUN cargo chef prepare --recipe-path recipe.json

# -----------------------------------------------------------------------------
# Stage 2: Dependency builder - Compiles all dependencies
# -----------------------------------------------------------------------------
# This stage is cached until Cargo.toml/Cargo.lock change.
# When only source code changes, Docker reuses this entire layer.
FROM chef AS deps

# Build arguments for metadata (not used in dep compilation, but declared early)
ARG VERSION=dev
ARG GIT_COMMIT=unknown

# Copy the recipe (dependency manifest)
COPY --from=planner /build/recipe.json recipe.json

# Install dependencies required for native compilation
# - pkg-config: For finding system libraries
# - libssl-dev: For TLS/crypto (if needed by dependencies)
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Compile dependencies only (source code is not copied yet)
# Uses --locked to respect Cargo.lock exactly
# Release mode for optimized binaries
RUN cargo chef cook --release --locked --recipe-path recipe.json

# -----------------------------------------------------------------------------
# Stage 3: Application builder - Compiles the actual application
# -----------------------------------------------------------------------------
FROM deps AS builder

ARG VERSION=dev
ARG GIT_COMMIT=unknown

# Copy full source code (dependencies are already compiled in previous stage)
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Build the release binary with:
# - --locked: Respect Cargo.lock exactly for reproducibility
# - --release: Optimization level 3, no debug symbols
# - RUSTFLAGS for additional optimizations:
#   - -C link-arg=-s: Strip symbols during linking (smaller binary)
ENV RUSTFLAGS="-C link-arg=-s"

# Embed version info at compile time (accessible via env! macro if implemented)
ENV NOVAI_VERSION=${VERSION}
ENV NOVAI_GIT_COMMIT=${GIT_COMMIT}

RUN cargo build --release --locked --bin novai-node

# Strip binary further for maximum size reduction
# Removes debug sections and symbol tables not needed at runtime
RUN strip --strip-all /build/target/release/novai-node

# Verify binary size
RUN ls -lh /build/target/release/novai-node

# -----------------------------------------------------------------------------
# Stage 4: Runtime - Minimal production image
# -----------------------------------------------------------------------------
# Using Google's distroless static image:
# - No shell, package manager, or other utilities (minimal attack surface)
# - Only contains the binary and CA certificates
# - Static variant for binaries that don't need dynamic linking
FROM gcr.io/distroless/static-debian12:nonroot AS runtime

# Build-time arguments for labels
ARG VERSION=dev
ARG GIT_COMMIT=unknown
ARG BUILD_DATE=unknown

# OCI-compliant labels for metadata
LABEL org.opencontainers.image.title="NOVAI Blockchain Node" \
      org.opencontainers.image.description="Production NOVAI consensus node" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${GIT_COMMIT}" \
      org.opencontainers.image.vendor="NOVAI Protocol" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.created="${BUILD_DATE}"

# Copy the stripped binary from builder
COPY --from=builder /build/target/release/novai-node /usr/local/bin/novai-node

# Volume mount point for node data (blocks, state, etc.)
# The nonroot image already has user 65532:65532 (nonroot:nonroot)
VOLUME ["/data"]

# Set working directory
WORKDIR /data

# Expose network ports:
# - 9090: P2P communication (TCP-based consensus messaging)
# - 8080: HTTP API (metrics, health checks - to be implemented in D9.6)
EXPOSE 9090 8080

# NOTE: HEALTHCHECK not implemented yet
# Distroless has no shell/wget, so health checks must be done via orchestration
# (Kubernetes readiness/liveness probes) or a separate health binary.
# Health endpoint will be added in D9.6 (metrics implementation).
#
# For Kubernetes, use:
#   livenessProbe:
#     tcpSocket:
#       port: 9090
#   readinessProbe:
#     httpGet:
#       path: /health
#       port: 8080

# Environment variables for runtime configuration
# These can be overridden at container start
ENV NOVAI_DATA_DIR=/data \
    NOVAI_P2P_PORT=9090 \
    NOVAI_HTTP_PORT=8080 \
    RUST_LOG=info

# Default entrypoint - the node binary
# Arguments can be passed at runtime:
#   docker run novai-node run --port 9090 --validator 0
ENTRYPOINT ["/usr/local/bin/novai-node"]

# Default command (can be overridden)
# Runs the node in default configuration
CMD ["run", "--port", "9090", "--validator", "0"]
