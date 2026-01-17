# NOVAI Node - Docker Build Instructions

## Quick Start

### Build the image

```bash
docker build \
  --build-arg VERSION=0.1.0 \
  --build-arg GIT_COMMIT=$(git rev-parse --short HEAD) \
  --build-arg BUILD_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ") \
  -t novai-node:latest \
  -t novai-node:0.1.0 \
  .
```

### Check image size (target: < 50MB)

```bash
docker images novai-node:latest
```

Expected output:
```
REPOSITORY    TAG       IMAGE ID       CREATED          SIZE
novai-node    latest    <hash>         <time>           ~3-5MB
```

### Run a single node

```bash
docker run -d \
  --name novai-validator-0 \
  -p 9090:9090 \
  -p 8080:8080 \
  -v novai-data:/data \
  novai-node:latest \
  run --port 9090 --validator 0
```

### View logs

```bash
docker logs -f novai-validator-0
```

### Stop the node

```bash
docker stop novai-validator-0
docker rm novai-validator-0
```

## Multi-Node Local Testnet

Run a 5-node devnet cluster locally:

```bash
# Node 0 (leader at height 0, round 0)
docker run -d \
  --name novai-0 \
  --network novai-net \
  -p 9000:9090 \
  -v novai-data-0:/data \
  novai-node:latest \
  run --port 9090 --validator 0

# Node 1
docker run -d \
  --name novai-1 \
  --network novai-net \
  -p 9001:9090 \
  -v novai-data-1:/data \
  novai-node:latest \
  run --port 9090 --peer novai-0:9090 --validator 1

# Node 2
docker run -d \
  --name novai-2 \
  --network novai-net \
  -p 9002:9090 \
  -v novai-data-2:/data \
  novai-node:latest \
  run --port 9090 --peer novai-0:9090 --peer novai-1:9090 --validator 2

# Node 3
docker run -d \
  --name novai-3 \
  --network novai-net \
  -p 9003:9090 \
  -v novai-data-3:/data \
  novai-node:latest \
  run --port 9090 --peer novai-0:9090 --validator 3

# Node 4
docker run -d \
  --name novai-4 \
  --network novai-net \
  -p 9004:9090 \
  -v novai-data-4:/data \
  novai-node:latest \
  run --port 9090 --peer novai-0:9090 --validator 4
```

Create the network first:
```bash
docker network create novai-net
```

View all node logs:
```bash
docker logs -f novai-0
docker logs -f novai-1
# etc.
```

Stop all nodes:
```bash
docker stop novai-0 novai-1 novai-2 novai-3 novai-4
docker rm novai-0 novai-1 novai-2 novai-3 novai-4
```

## Image Details

### Architecture
- **Base**: Google Distroless Static (Debian 12)
- **User**: nonroot (UID 65532)
- **Size**: ~3-5MB (well under 50MB target)
- **Security**: No shell, minimal attack surface

### Build Stages
1. **Chef Planner**: Extracts dependency graph
2. **Dependency Builder**: Compiles all dependencies (cached)
3. **Application Builder**: Compiles novai-node binary
4. **Runtime**: Minimal distroless image with binary only

### Build Arguments
- `VERSION`: Semantic version (default: `dev`)
- `GIT_COMMIT`: Git commit hash (default: `unknown`)
- `BUILD_DATE`: ISO 8601 build timestamp (default: `unknown`)

### Exposed Ports
- `9090`: P2P consensus messaging (TCP)
- `8080`: HTTP API (metrics/health - to be implemented in D9.6)

### Environment Variables
- `NOVAI_DATA_DIR=/data`: Data directory path
- `NOVAI_P2P_PORT=9090`: P2P listen port
- `NOVAI_HTTP_PORT=8080`: HTTP listen port
- `RUST_LOG=info`: Log level (debug, info, warn, error)

### Volumes
- `/data`: Persistent storage for blocks, state, and configuration

## Advanced Usage

### Override entrypoint

```bash
# Note: distroless has no shell, so you can't use /bin/sh
docker run -it novai-node:latest --help
```

### Inspect image metadata

```bash
docker inspect novai-node:latest | jq '.[0].Config.Labels'
```

### Build with custom Rust toolchain (for testing)

Edit `Dockerfile` and change:
```dockerfile
FROM rust:1.84.0-bookworm AS chef
```

### Push to registry

```bash
# Tag for your registry
docker tag novai-node:latest ghcr.io/your-org/novai-node:0.1.0

# Push
docker push ghcr.io/your-org/novai-node:0.1.0
```

## Troubleshooting

### Build fails at cargo-chef install

Increase Docker memory allocation to at least 4GB.

### Binary size too large

Check that stripping is working:
```bash
# During build, you should see output like:
# -rwxr-xr-x 1 root root 600K ... /build/target/release/novai-node
```

If binary is > 1MB after stripping, check RUSTFLAGS.

### Container exits immediately

Check logs:
```bash
docker logs novai-validator-0
```

Common issues:
- Invalid --validator index
- Port already in use
- Missing --peer arguments for non-leader nodes

### Permission denied in /data

The distroless image runs as user `nonroot` (UID 65532). Ensure volume permissions:
```bash
# If using bind mount
sudo chown -R 65532:65532 /path/to/data
```

## Health Checks

Health endpoint (`/health` on port 8080) will be implemented in D9.6.

For now, use TCP checks:

**Docker Compose:**
```yaml
healthcheck:
  test: ["CMD-SHELL", "timeout 1 bash -c 'cat < /dev/null > /dev/tcp/localhost/9090'"]
  interval: 30s
  timeout: 5s
  retries: 3
  start_period: 10s
```

**Kubernetes:**
```yaml
livenessProbe:
  tcpSocket:
    port: 9090
  initialDelaySeconds: 10
  periodSeconds: 30

readinessProbe:
  tcpSocket:
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
```

## Build Performance

First build:
- ~2-3 minutes (compiles all dependencies + application)

Subsequent builds (code changes only):
- ~30 seconds (cargo-chef caches dependencies)

To force full rebuild:
```bash
docker build --no-cache -t novai-node:latest .
```

## Image Optimization

Current optimizations:
- ✅ Multi-stage build (removes build tools from final image)
- ✅ cargo-chef (dependency caching)
- ✅ RUSTFLAGS="-C link-arg=-s" (strip symbols during linking)
- ✅ strip --strip-all (remove debug sections)
- ✅ distroless-static (minimal base image)

Potential future optimizations:
- UPX compression (risky, can cause runtime issues)
- Profile-guided optimization (PGO)
- Link-time optimization (LTO) - already enabled in release mode

## Security

### Non-root user
Container runs as `nonroot` (UID 65532), not root.

### No shell
Distroless image contains no shell or package manager, reducing attack surface.

### Minimal dependencies
Only the binary and CA certificates are included.

### Version pinning
Rust toolchain and cargo-chef versions are pinned for reproducibility.

### Supply chain
Base images from official sources:
- `rust:1.84.0-bookworm` (official Rust image)
- `gcr.io/distroless/static-debian12:nonroot` (Google Distroless)

## Reproducibility

To verify reproducible builds:

```bash
# Build 1
docker build --build-arg VERSION=test --build-arg GIT_COMMIT=abc123 -t novai-test-1 .

# Build 2 (same args)
docker build --build-arg VERSION=test --build-arg GIT_COMMIT=abc123 -t novai-test-2 .

# Compare images (should be identical)
docker images --digests | grep novai-test
```

Note: Timestamps in labels will differ, but binary digest should match.
