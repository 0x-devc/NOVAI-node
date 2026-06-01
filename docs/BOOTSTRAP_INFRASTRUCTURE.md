# NOVAI Bootstrapping Infrastructure

**Version**: 1.0.0
**Status**: DRAFT
**Last Updated**: 2026-02-03

This document defines the infrastructure required to bootstrap the NOVAI mainnet.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Seed Nodes](#2-seed-nodes)
3. [RPC Endpoints](#3-rpc-endpoints)
4. [Genesis Distribution](#4-genesis-distribution)
5. [Snapshot Distribution](#5-snapshot-distribution)
6. [DNS Configuration](#6-dns-configuration)
7. [Infrastructure Checklist](#7-infrastructure-checklist)

---

## 1. Overview

### 1.1 Purpose

Bootstrapping infrastructure enables new nodes to:

1. **Discover peers** via seed nodes
2. **Verify genesis** via distributed genesis files
3. **Sync state** via snapshots (optional, for fast sync)
4. **Query chain** via public RPC endpoints

### 1.2 Availability Requirements

| Component | Availability Target | Redundancy |
|-----------|---------------------|------------|
| Seed nodes | 99.9% | 3-5 nodes, geographically distributed |
| RPC endpoints | 99.5% | 2+ nodes behind load balancer |
| Genesis files | 99.99% | CDN + multiple mirrors |
| Snapshots | 99% | Object storage + CDN |

---

## 2. Seed Nodes

### 2.1 Seed Node List

Seed nodes are the first point of contact for new validators joining the network.

| Seed Node | Region | Hostname | IP Address | Port |
|-----------|--------|----------|------------|------|
| seed-1 | US East (Virginia) | `seed-1.mainnet.novai.io` | TBD | 9090 |
| seed-2 | EU West (Frankfurt) | `seed-2.mainnet.novai.io` | TBD | 9090 |
| seed-3 | Asia Pacific (Singapore) | `seed-3.mainnet.novai.io` | TBD | 9090 |
| seed-4 | US West (Oregon) | `seed-4.mainnet.novai.io` | TBD | 9090 |
| seed-5 | EU North (Stockholm) | `seed-5.mainnet.novai.io` | TBD | 9090 |

### 2.2 Seed Node Requirements

Each seed node must meet these requirements:

| Requirement | Specification |
|-------------|---------------|
| Uptime | 99.9% (max 8.7 hours downtime/year) |
| Bandwidth | 1 Gbps symmetric |
| Connections | Support 500+ concurrent peers |
| DDoS protection | Required (cloud provider or dedicated) |
| Monitoring | 24/7 with alerting |

### 2.3 Seed Node Configuration

Seed nodes run in **non-validator mode** with special configuration:

```toml
# /etc/novai/config/seed-node.toml

[node]
chain_id = "novai-mainnet-1"
mode = "seed"  # Non-validator, peer discovery only

[network]
listen_addr = "0.0.0.0:9090"
public_addr = "seed-1.mainnet.novai.io:9090"

# No seed nodes for seed nodes (they ARE the seeds)
seed_nodes = []

# High peer limit for seed nodes
max_peers = 500
max_inbound_peers = 400
max_outbound_peers = 100

# Peer exchange enabled
enable_peer_exchange = true

# Aggressive peer discovery
peer_discovery_interval = "10s"

[metrics]
listen_addr = "127.0.0.1:8080"
enabled = true
```

### 2.4 Seed Node Deployment

**Recommended providers:**

| Provider | Service | Region Coverage |
|----------|---------|-----------------|
| AWS | EC2 c6i.xlarge | Global |
| GCP | n2-standard-4 | Global |
| Vultr | High Frequency | Global |

**Deployment checklist per seed node:**

- [ ] Provision server meeting requirements
- [ ] Configure firewall (allow 9090/tcp inbound)
- [ ] Install NOVAI node software
- [ ] Configure as seed node (see above)
- [ ] Set up monitoring and alerting
- [ ] Configure DNS record
- [ ] Test connectivity from multiple regions
- [ ] Document in seed node registry

### 2.5 Seed Node Monitoring

Monitor these metrics for each seed node:

| Metric | Alert Threshold |
|--------|-----------------|
| `novai_peer_count` | < 10 (warning), < 5 (critical) |
| `novai_inbound_connections` | > 450 (warning, approaching limit) |
| Network bandwidth | > 800 Mbps sustained |
| CPU usage | > 80% sustained |
| Memory usage | > 80% |

---

## 3. RPC Endpoints

### 3.1 Public RPC Endpoints

| Endpoint | Purpose | Rate Limit |
|----------|---------|------------|
| `https://rpc.mainnet.novai.io` | Primary RPC | 100 req/s per IP |
| `https://rpc-backup.mainnet.novai.io` | Backup RPC | 50 req/s per IP |

### 3.2 RPC Methods

Available JSON-RPC methods:

| Method | Description | Auth Required |
|--------|-------------|---------------|
| `chain_getBlock` | Get block by height | No |
| `chain_getLatestBlock` | Get latest block | No |
| `chain_getTransaction` | Get transaction by ID | No |
| `state_getAccount` | Get account balance/nonce | No |
| `state_getStateRoot` | Get current state root | No |
| `mempool_submit` | Submit transaction | No |
| `mempool_getStatus` | Get mempool status | No |
| `node_getHealth` | Health check | No |
| `node_getMetrics` | Node metrics | No |

### 3.3 RPC Infrastructure

```
                    ┌─────────────────┐
                    │   Cloudflare    │
                    │   (DDoS/CDN)    │
                    │   Port 443      │
                    └────────┬────────┘
                             │
                    ┌────────▼────────┐
                    │  Load Balancer  │
                    │  (nginx/HAProxy)│
                    └────────┬────────┘
                             │
          ┌──────────────────┼──────────────────┐
          │                  │                  │
   ┌──────▼──────┐    ┌──────▼──────┐    ┌──────▼──────┐
   │  RPC Node 1 │    │  RPC Node 2 │    │  RPC Node 3 │
   │  (us-east)  │    │  (eu-west)  │    │  (ap-south) │
   │  Port 8545  │    │  Port 8545  │    │  Port 8545  │
   └─────────────┘    └─────────────┘    └─────────────┘
```

**TLS Termination**: TLS is terminated at the Cloudflare edge. Traffic between Cloudflare and the load balancer uses origin certificates. RPC nodes accept plaintext HTTP on port 8545 from the load balancer only.

### 3.4 RPC Node Configuration

```toml
# /etc/novai/config/rpc-node.toml
#
# Architecture notes:
# - Public access: HTTPS on port 443 (TLS terminated at Cloudflare)
# - Load balancer: Receives traffic from Cloudflare, distributes to RPC nodes
# - RPC nodes: Listen on port 8545 (plaintext HTTP, internal only)
# - Firewall: Port 8545 only accessible from load balancer IP

[node]
chain_id = "novai-mainnet-1"
mode = "full"  # Full node, not validator

[network]
listen_addr = "0.0.0.0:9090"
seed_nodes = [
    "seed-1.mainnet.novai.io:9090",
    "seed-2.mainnet.novai.io:9090",
    "seed-3.mainnet.novai.io:9090",
]
max_peers = 50

[rpc]
enabled = true
listen_addr = "127.0.0.1:8545"  # Internal only, behind reverse proxy
max_connections = 1000
rate_limit_per_ip = 100

[metrics]
listen_addr = "127.0.0.1:8080"
enabled = true
```

---

## 4. Genesis Distribution

### 4.1 Genesis Files

| File | Description | Hash (blake3) |
|------|-------------|---------------|
| `genesis_config.json` | Genesis configuration | TBD (post-ceremony) |
| `genesis_block.bin` | Binary genesis block | TBD (post-ceremony) |
| `state_root.hex` | Genesis state root | TBD (post-ceremony) |
| `validator_set.json` | Initial validator set | TBD (post-ceremony) |

### 4.2 Distribution URLs

**Primary (CDN):**
```
https://mainnet.novai.io/genesis/genesis_config.json
https://mainnet.novai.io/genesis/genesis_block.bin
https://mainnet.novai.io/genesis/state_root.hex
https://mainnet.novai.io/genesis/validator_set.json
https://mainnet.novai.io/genesis/checksums.txt
```

**Mirrors:**
```
https://github.com/novai-protocol/mainnet-genesis/releases/latest
https://ipfs.io/ipfs/<CID>  # IPFS for decentralized access
```

### 4.3 Verification Procedure

New validators must verify genesis before starting:

```bash
# Download genesis files
curl -O https://mainnet.novai.io/genesis/genesis_config.json
curl -O https://mainnet.novai.io/genesis/genesis_block.bin
curl -O https://mainnet.novai.io/genesis/state_root.hex
curl -O https://mainnet.novai.io/genesis/checksums.txt

# Verify checksums
blake3sum -c checksums.txt

# Verify state root matches
genesis-generator --config genesis_config.json --verify $(cat state_root.hex)
```

### 4.4 Checksums File Format

```
# checksums.txt (blake3)
<hash>  genesis_config.json
<hash>  genesis_block.bin
<hash>  state_root.hex
<hash>  validator_set.json
```

---

## 5. Snapshot Distribution

### 5.1 Overview

Snapshots allow new nodes to sync quickly without replaying all blocks from genesis.

**Snapshot frequency**: Daily at 00:00 UTC
**Retention**: 7 days

### 5.2 Snapshot Contents

```
novai-snapshot-<height>-<date>.tar.gz
├── state/           # Full state database
├── blocks/          # Block headers (last 1000)
├── metadata.json    # Snapshot metadata
└── checksum.blake3  # File integrity
```

### 5.3 Metadata Format

```json
{
    "version": 1,
    "chain_id": "novai-mainnet-1",
    "height": 1234567,
    "state_root": "<64-hex-chars>",
    "timestamp": "2026-03-15T00:00:00Z",
    "size_bytes": 10737418240,
    "checksum_blake3": "<64-hex-chars>"
}
```

### 5.4 Snapshot URLs

```
https://snapshots.mainnet.novai.io/latest/
https://snapshots.mainnet.novai.io/archive/<date>/
```

### 5.5 Snapshot Restoration

```bash
# Download latest snapshot
curl -O https://snapshots.mainnet.novai.io/latest/novai-snapshot-latest.tar.gz
curl -O https://snapshots.mainnet.novai.io/latest/metadata.json

# Verify checksum
EXPECTED=$(jq -r .checksum_blake3 metadata.json)
ACTUAL=$(blake3sum novai-snapshot-latest.tar.gz | awk '{print $1}')
if [ "$EXPECTED" != "$ACTUAL" ]; then
    echo "Checksum mismatch!"
    exit 1
fi

# Stop node
sudo systemctl stop novai-validator

# Restore snapshot
sudo rm -rf /var/lib/novai/*
sudo tar xzf novai-snapshot-latest.tar.gz -C /var/lib/novai/

# Start node (will catch up remaining blocks)
sudo systemctl start novai-validator
```

### 5.6 Snapshot Infrastructure

| Component | Provider | Purpose |
|-----------|----------|---------|
| Storage | AWS S3 / GCS | Snapshot storage |
| CDN | Cloudflare R2 | Global distribution |
| Generator | Dedicated server | Daily snapshot creation |

**Note**: Snapshot generation automation is defined in `docs/OPERATOR_RUNBOOK.md` Section 6 (Data Management). The generation script runs on a dedicated full node and uploads to object storage via cron at 00:00 UTC daily.

---

## 6. DNS Configuration

### 6.1 DNS Records

| Record | Type | Value | TTL |
|--------|------|-------|-----|
| `mainnet.novai.io` | A | CDN IP | 300 |
| `seed-1.mainnet.novai.io` | A | Seed node 1 IP | 60 |
| `seed-2.mainnet.novai.io` | A | Seed node 2 IP | 60 |
| `seed-3.mainnet.novai.io` | A | Seed node 3 IP | 60 |
| `seed-4.mainnet.novai.io` | A | Seed node 4 IP | 60 |
| `seed-5.mainnet.novai.io` | A | Seed node 5 IP | 60 |
| `rpc.mainnet.novai.io` | CNAME | Load balancer | 300 |
| `snapshots.mainnet.novai.io` | CNAME | CDN | 300 |

### 6.2 DNS Provider Requirements

- DNSSEC enabled
- Low TTL support (60 seconds minimum)
- Anycast or GeoDNS for global distribution
- 99.99% availability SLA

### 6.3 Failover Configuration

Use health-check-based DNS failover:

```
seed-1.mainnet.novai.io
├── Primary: 1.2.3.4 (us-east)
└── Failover: 5.6.7.8 (eu-west) [if primary health check fails]
```

---

## 7. Infrastructure Checklist

### 7.1 Pre-Launch (T-7 days)

- [ ] All 5 seed nodes provisioned and configured
- [ ] Seed node DNS records configured
- [ ] Seed node monitoring and alerting active
- [ ] RPC infrastructure deployed
- [ ] RPC load balancer configured
- [ ] CDN configured for genesis distribution
- [ ] Snapshot infrastructure ready (can be empty initially)

### 7.2 Launch Day (T+0)

- [ ] Genesis files uploaded to CDN
- [ ] Genesis checksums published
- [ ] Seed nodes started with genesis
- [ ] RPC nodes started with genesis
- [ ] DNS records verified
- [ ] Connectivity tested from multiple regions

### 7.3 Post-Launch (T+1 to T+7)

- [ ] First snapshot generated and uploaded
- [ ] Snapshot automation verified
- [ ] Monitor seed node peer counts
- [ ] Monitor RPC request rates
- [ ] Address any connectivity issues reported

### 7.4 Ongoing Operations

| Task | Frequency | Owner |
|------|-----------|-------|
| Seed node health check | Every 5 min | Automated |
| RPC health check | Every 1 min | Automated |
| Snapshot generation | Daily | Automated |
| Snapshot cleanup (>7 days) | Daily | Automated |
| Security updates | Weekly | Ops team |
| Capacity review | Monthly | Ops team |

---

## Appendix A: Quick Reference

### Seed Nodes

```
seed-1.mainnet.novai.io:9090  # US East
seed-2.mainnet.novai.io:9090  # EU West
seed-3.mainnet.novai.io:9090  # Asia Pacific
seed-4.mainnet.novai.io:9090  # US West
seed-5.mainnet.novai.io:9090  # EU North
```

### RPC Endpoints

```
https://rpc.mainnet.novai.io        # Primary (HTTPS, port 443 implied)
https://rpc-backup.mainnet.novai.io # Backup (HTTPS, port 443 implied)
```

### Genesis Files

```
https://mainnet.novai.io/genesis/genesis_config.json
https://mainnet.novai.io/genesis/genesis_block.bin
https://mainnet.novai.io/genesis/state_root.hex
https://mainnet.novai.io/genesis/checksums.txt
```

### Snapshots

```
https://snapshots.mainnet.novai.io/latest/
```

---

## Appendix B: Contact Information

| Role | Contact | Escalation |
|------|---------|------------|
| Infrastructure Lead | [TBD] | [TBD] |
| On-call Engineer | [TBD] | PagerDuty |
| Security Team | [TBD] | [TBD] |

---

**End of Bootstrapping Infrastructure**
