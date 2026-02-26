# NOVAI Validator Kit

**Version**: 1.0.0
**Status**: DRAFT
**Last Updated**: 2026-02-03

Complete guide for setting up and operating a NOVAI mainnet validator node.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Hardware Requirements](#2-hardware-requirements)
3. [Operating System Setup](#3-operating-system-setup)
4. [Key Management](#4-key-management)
5. [Node Installation](#5-node-installation)
6. [Configuration](#6-configuration)
7. [Monitoring Setup](#7-monitoring-setup)
8. [Backup Procedures](#8-backup-procedures)
9. [Upgrade Procedure](#9-upgrade-procedure)
10. [Security Hardening](#10-security-hardening)
11. [Troubleshooting](#11-troubleshooting)

---

## 1. Overview

### 1.1 Validator Responsibilities

As a NOVAI validator, you are responsible for:

- **Proposing blocks** when selected as leader
- **Voting on proposals** from other validators
- **Maintaining uptime** (target: 99.9%)
- **Securing your validator key** (see [Key Management](#4-key-management))
- **Keeping software updated** (see [Upgrade Procedure](#9-upgrade-procedure))

### 1.2 Network Parameters

| Parameter | Value | Source |
|-----------|-------|--------|
| Block time target | ~10 seconds | Consensus configuration |
| BFT fault tolerance | f < n/3 | `crates/consensus_types/src/leader.rs` |
| Quorum threshold | 2f + 1 | `crates/consensus_types/src/leader.rs` |
| Minimum validators | 4 (n = 3f + 1, f = 1) | Protocol requirement |
| Maximum validators | 100 | `crates/genesis/src/lib.rs:217` |

### 1.3 Staking Requirements

| Requirement | Value | Notes |
|-------------|-------|-------|
| Minimum stake | TBD | Set in genesis config |
| Unbonding period | TBD | Governance parameter |
| Slashing conditions | TBD | See security documentation |

---

## 2. Hardware Requirements

### 2.1 Minimum Specifications

| Component | Minimum | Notes |
|-----------|---------|-------|
| **CPU** | 4 cores, 2.5 GHz | x86_64 or ARM64 |
| **RAM** | 8 GB | 16 GB recommended |
| **Storage** | 500 GB SSD | NVMe preferred |
| **Network** | 100 Mbps | Symmetric up/down |
| **Public IP** | Required | Static preferred |

### 2.2 Recommended Specifications

| Component | Recommended | Notes |
|-----------|-------------|-------|
| **CPU** | 8+ cores, 3.0+ GHz | AMD EPYC or Intel Xeon |
| **RAM** | 32 GB | ECC recommended |
| **Storage** | 1 TB NVMe SSD | Enterprise-grade (e.g., Samsung PM9A3) |
| **Network** | 1 Gbps | Low latency to peers |
| **Public IP** | Static IPv4 | IPv6 optional |

### 2.3 Storage Considerations

**Current state size**: ~10 GB (testnet estimate)
**Growth rate**: ~1-5 GB/month (depends on transaction volume)
**Recommended headroom**: 2x current size minimum

```
Storage breakdown:
├── /data/blocks/     # Block storage (~60% of total)
├── /data/state/      # State database (~30% of total)
├── /data/logs/       # Application logs (~5% of total)
└── /data/snapshots/  # Periodic snapshots (~5% of total)
```

### 2.4 Network Requirements

| Port | Protocol | Direction | Purpose |
|------|----------|-----------|---------|
| 9090 | TCP | Inbound/Outbound | P2P consensus |
| 8080 | TCP | Inbound (optional) | Metrics endpoint |

**Bandwidth estimates:**

| Activity | Bandwidth |
|----------|-----------|
| Idle (connected) | ~10 KB/s |
| Active consensus | ~100-500 KB/s |
| Catching up | ~5-10 MB/s |
| Peak load | ~1-2 MB/s |

### 2.5 Cloud Provider Recommendations

| Provider | Instance Type | Monthly Cost (est.) |
|----------|--------------|---------------------|
| AWS | m6i.xlarge | ~$150 |
| GCP | n2-standard-4 | ~$140 |
| Azure | Standard_D4s_v3 | ~$145 |
| [REDACTED] | AX41-NVMe | ~$50 |
| OVH | Advance-1 | ~$60 |

**Note**: Costs are estimates. Always verify current pricing.

---

## 3. Operating System Setup

### 3.1 Supported Operating Systems

| OS | Version | Support Level |
|----|---------|---------------|
| Ubuntu | 22.04 LTS | Primary (recommended) |
| Ubuntu | 24.04 LTS | Supported |
| Debian | 12 (Bookworm) | Supported |
| Rocky Linux | 9 | Supported |
| Docker | Any host with Docker 24+ | Container deployment |

### 3.2 Ubuntu 22.04 LTS Setup

**Step 1: Initial system update**

```bash
sudo apt update && sudo apt upgrade -y
sudo reboot
```

**Step 2: Install required packages**

```bash
sudo apt install -y \
    build-essential \
    curl \
    git \
    jq \
    htop \
    iotop \
    net-tools \
    ufw \
    fail2ban \
    unattended-upgrades
```

**Step 3: Configure automatic security updates**

```bash
sudo dpkg-reconfigure -plow unattended-upgrades
# Select "Yes" for automatic updates
```

**Step 4: Create dedicated user**

```bash
sudo useradd -m -s /bin/bash novai
sudo usermod -aG sudo novai
sudo passwd novai
```

**Step 5: Configure firewall**

```bash
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow ssh
sudo ufw allow 9090/tcp comment "NOVAI P2P"
sudo ufw allow from 127.0.0.1 to any port 8080 proto tcp comment "NOVAI Metrics (local)"
sudo ufw enable
```

**Step 6: Configure fail2ban**

```bash
sudo systemctl enable fail2ban
sudo systemctl start fail2ban
```

### 3.3 System Tuning

**Increase file descriptor limits:**

```bash
# /etc/security/limits.conf
novai soft nofile 65536
novai hard nofile 65536
```

**Optimize network settings:**

```bash
# /etc/sysctl.d/99-novai.conf
net.core.somaxconn = 65535
net.core.netdev_max_backlog = 65535
net.ipv4.tcp_max_syn_backlog = 65535
net.ipv4.tcp_fin_timeout = 30
net.ipv4.tcp_keepalive_time = 300
net.ipv4.tcp_keepalive_probes = 5
net.ipv4.tcp_keepalive_intvl = 15
```

Apply settings:

```bash
sudo sysctl -p /etc/sysctl.d/99-novai.conf
```

### 3.4 Time Synchronization

**Critical**: Validators must have accurate time.

```bash
sudo apt install -y chrony
sudo systemctl enable chrony
sudo systemctl start chrony

# Verify synchronization
chronyc tracking
```

Expected output should show `Leap status: Normal` and low offset.

---

## 4. Key Management

### 4.1 Key Types

| Key | Purpose | Storage |
|-----|---------|---------|
| Validator signing key | Sign proposals and votes | HSM (recommended) |
| Node identity key | P2P authentication | Encrypted file |

### 4.2 HSM Setup (Recommended)

See `docs/GENESIS_CEREMONY.md` Section 4 for detailed HSM setup instructions.

**Supported HSMs:**

| HSM | Setup Guide |
|-----|-------------|
| YubiHSM 2 | [YubiHSM 2 Setup](#yubihsm-2-setup) |
| AWS CloudHSM | [CloudHSM Setup](#aws-cloudhsm-setup) |

#### YubiHSM 2 Setup

```bash
# Install YubiHSM tools
wget https://developers.yubico.com/YubiHSM2/Releases/yubihsm2-sdk-2023.08-ubuntu2204-amd64.tar.gz
tar xzf yubihsm2-sdk-*.tar.gz
sudo dpkg -i yubihsm2-sdk/*.deb

# Start connector
sudo systemctl enable yubihsm-connector
sudo systemctl start yubihsm-connector

# Generate validator key (see GENESIS_CEREMONY.md for details)
yubihsm-shell -a generate-asymmetric \
    --object-id 100 \
    --label "novai-mainnet-validator" \
    --algorithm ed25519 \
    --capabilities sign-eddsa
```

### 4.3 Software Key Storage (Fallback)

**WARNING**: Only use if HSM is unavailable.

```bash
# Create secure key directory
sudo mkdir -p /etc/novai/keys
sudo chown novai:novai /etc/novai/keys
sudo chmod 700 /etc/novai/keys

# Generate key (on air-gapped machine, transfer securely)
# See GENESIS_CEREMONY.md Section 4.3

# Encrypt key file
gpg --symmetric --cipher-algo AES256 validator_key.pem
# Move encrypted key to validator
sudo mv validator_key.pem.gpg /etc/novai/keys/

# Decrypt at startup (manual or via secure script)
gpg --decrypt /etc/novai/keys/validator_key.pem.gpg > /tmp/validator_key.pem
# Use key, then securely delete
shred -u /tmp/validator_key.pem
```

### 4.4 Key Backup

- [ ] Create encrypted backup of private key
- [ ] Store backup in geographically separate location
- [ ] Test backup restoration annually
- [ ] Document backup location (offline, secure)

---

## 5. Node Installation

### 5.1 Option A: Docker Installation (Recommended)

**Step 1: Install Docker**

```bash
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker novai
# Log out and back in for group membership
```

**Step 2: Pull NOVAI node image**

```bash
docker pull novai/novai-node:mainnet-v1.0.0
```

**Step 3: Verify image signature**

```bash
# Verify image digest matches published value
docker inspect novai/novai-node:mainnet-v1.0.0 --format='{{.RepoDigests}}'
# Compare with: https://github.com/novai-protocol/novai-node/releases
```

**Step 4: Create data directory**

```bash
sudo mkdir -p /var/lib/novai
sudo chown novai:novai /var/lib/novai
```

**Step 5: Create systemd service**

```bash
sudo tee /etc/systemd/system/novai-validator.service << 'EOF'
[Unit]
Description=NOVAI Validator Node
After=docker.service
Requires=docker.service

[Service]
User=novai
Group=novai
Type=simple
Restart=always
RestartSec=10
TimeoutStartSec=300
TimeoutStopSec=60

ExecStartPre=-/usr/bin/docker stop novai-validator
ExecStartPre=-/usr/bin/docker rm novai-validator

ExecStart=/usr/bin/docker run \
    --name novai-validator \
    --network host \
    -v /var/lib/novai:/data \
    -v /etc/novai/keys:/keys:ro \
    -v /etc/novai/genesis:/genesis:ro \
    novai/novai-node:latest \
    run \
    --port 9090 \
    --data-dir /data \
    --genesis /genesis/genesis.json \
    --key-file /keys/validator_key.pem \
    --metrics-port 8080 \
    --storage rocksdb

ExecStop=/usr/bin/docker stop novai-validator

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable novai-validator
```

### 5.2 Option B: Binary Installation

**Step 1: Install Rust**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

**Step 2: Clone and build**

```bash
git clone https://github.com/novai-protocol/novai-node.git
cd novai-node
git checkout mainnet-v1.0.0
git verify-tag mainnet-v1.0.0

cargo build --release
sudo cp target/release/novai-node /usr/local/bin/
```

**Step 3: Create systemd service**

```bash
sudo tee /etc/systemd/system/novai-validator.service << 'EOF'
[Unit]
Description=NOVAI Validator Node
After=network-online.target
Wants=network-online.target

[Service]
User=novai
Group=novai
Type=simple
Restart=always
RestartSec=10
LimitNOFILE=65536

ExecStart=/usr/local/bin/novai-node run \
    --port 9090 \
    --data-dir /var/lib/novai \
    --genesis /etc/novai/genesis/genesis.json \
    --key-file /etc/novai/keys/validator_key.pem \
    --metrics-port 8080 \
    --storage rocksdb

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable novai-validator
```

### 5.3 Genesis Setup

```bash
# Download genesis files
curl -O https://mainnet.novai.io/genesis/genesis_config.json
curl -O https://mainnet.novai.io/genesis/genesis_block.bin
curl -O https://mainnet.novai.io/genesis/state_root.hex

# Verify state root
genesis-generator --config genesis_config.json --verify $(cat state_root.hex)
# Expected: VERIFICATION PASSED

# Install genesis
sudo mkdir -p /etc/novai/genesis
sudo cp genesis_config.json genesis_block.bin /etc/novai/genesis/
sudo chown -R novai:novai /etc/novai
```

---

## 6. Configuration

### 6.1 CLI Flags Reference

NOVAI node is configured entirely via CLI flags (no TOML config file):

```
novai-node run \
    --port <port>                    # P2P listen port (required)
    --genesis <path>                 # Path to genesis JSON (required unless --dev-keys)
    --key-file <path>                # Path to validator Ed25519 key file
    --peer <addr>                    # Peer address (repeatable: --peer a --peer b)
    --metrics-port <port>            # Prometheus metrics port (default: none)
    --base-timeout <ms>              # Consensus timeout in ms (default: 1000)
    --proposal-interval <ms>         # Min ms between proposals (default: 100, min: 20)
    --storage <rocksdb|memory>       # Storage backend (default: rocksdb)
    --data-dir <path>                # Data directory for RocksDB
    --no-encryption                  # Disable Noise XX transport (testing only)
```

**Dev mode** (deterministic keys for local testing):
```
novai-node run \
    --port 9000 \
    --dev-keys --allow-insecure-dev-keys \
    --validator <index>              # Validator index (0-based)
```

**Other commands**:
```
novai-node generate-key --output <path>    # Generate Ed25519 validator key
novai-node submit-tx <payload>             # Submit a transaction
novai-node drain-mempool <payload>...      # Batch submit transactions
```

**Environment variables** for logging:
```bash
export RUST_LOG=info                # Log level: error, warn, info, debug, trace
```

### 6.2 Start the Node

```bash
sudo systemctl start novai-validator
sudo systemctl status novai-validator

# View logs
sudo journalctl -u novai-validator -f
```

### 6.3 Verify Node is Running

```bash
# Check health
curl http://localhost:8080/health
# Expected: OK

# Check metrics
curl http://localhost:8080/metrics | grep committed_height

# Check peer connections
curl http://localhost:8080/metrics | grep peer_count
```

---

## 7. Monitoring Setup

### 7.1 Prometheus Configuration

**Install Prometheus:**

```bash
sudo apt install -y prometheus
```

**Configure scraping (`/etc/prometheus/prometheus.yml`):**

```yaml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: 'novai-validator'
    static_configs:
      - targets: ['localhost:8080']
        labels:
          instance: 'validator'
```

**Start Prometheus:**

```bash
sudo systemctl enable prometheus
sudo systemctl start prometheus
```

### 7.2 Grafana Dashboard

**Install Grafana:**

```bash
sudo apt install -y apt-transport-https software-properties-common
wget -q -O - https://packages.grafana.com/gpg.key | sudo apt-key add -
echo "deb https://packages.grafana.com/oss/deb stable main" | sudo tee /etc/apt/sources.list.d/grafana.list
sudo apt update
sudo apt install -y grafana

sudo systemctl enable grafana-server
sudo systemctl start grafana-server
```

**Import NOVAI dashboard:**

1. Open Grafana: `http://localhost:3000` (default: admin/admin)
2. Add Prometheus data source: `http://localhost:9090`
3. Import dashboard from `dashboards/novai-validator.json`

### 7.3 Key Metrics to Monitor

| Metric | Normal | Warning | Critical |
|--------|--------|---------|----------|
| `novai_committed_height` | Increasing | Stalled >30s | Stalled >60s |
| `novai_current_round` | 0-1 | 2-4 | 5+ |
| `novai_peer_count` | 4+ | 2-3 | <2 |
| `novai_mempool_size` | 0-500 | 500-800 | >800 |

### 7.4 Alerting

See `docs/OPERATOR_RUNBOOK.md` Section 4 for complete alerting configuration.

**Critical alerts to configure:**

- Consensus stalled (no new blocks in 60s)
- Insufficient peers (<3)
- High round number (>5)
- Node offline (health check fails)

---

## 8. Backup Procedures

### 8.1 What to Backup

| Item | Location | Frequency | Method |
|------|----------|-----------|--------|
| Validator key | `/etc/novai/keys/` | Once (at creation) | Encrypted offline |
| Node config | `/etc/novai/config/` | On change | Version control |
| State data | `/var/lib/novai/` | Daily | Snapshot |

### 8.2 State Backup Script

```bash
#!/bin/bash
# /usr/local/bin/novai-backup.sh

BACKUP_DIR="/var/backups/novai"
DATE=$(date +%Y%m%d-%H%M%S)
RETENTION_DAYS=7

# Stop node for consistent backup
sudo systemctl stop novai-validator

# Create backup
mkdir -p "$BACKUP_DIR"
tar czf "$BACKUP_DIR/novai-state-$DATE.tar.gz" /var/lib/novai

# Restart node
sudo systemctl start novai-validator

# Cleanup old backups
find "$BACKUP_DIR" -name "novai-state-*.tar.gz" -mtime +$RETENTION_DAYS -delete

echo "Backup completed: $BACKUP_DIR/novai-state-$DATE.tar.gz"
```

**Schedule daily backup:**

```bash
sudo crontab -e
# Add:
0 3 * * * /usr/local/bin/novai-backup.sh >> /var/log/novai-backup.log 2>&1
```

### 8.3 Restore Procedure

```bash
# Stop node
sudo systemctl stop novai-validator

# Restore from backup
sudo rm -rf /var/lib/novai/*
sudo tar xzf /var/backups/novai/novai-state-YYYYMMDD-HHMMSS.tar.gz -C /

# Start node (will catch up from peers)
sudo systemctl start novai-validator

# Monitor catch-up progress
sudo journalctl -u novai-validator -f | grep -E "(RECOVERY|catch-up)"
```

---

## 9. Upgrade Procedure

### 9.1 Pre-Upgrade Checklist

- [ ] Read release notes for breaking changes
- [ ] Backup current state (see Section 8)
- [ ] Verify new version on testnet first
- [ ] Coordinate with other validators (if consensus-breaking)
- [ ] Schedule maintenance window

### 9.2 Docker Upgrade

```bash
# Pull new image
docker pull novai/novai-node:mainnet-v1.1.0

# Verify image
docker inspect novai/novai-node:mainnet-v1.1.0 --format='{{.RepoDigests}}'

# Update service file with new version
sudo sed -i 's/mainnet-v1.0.0/mainnet-v1.1.0/' /etc/systemd/system/novai-validator.service
sudo systemctl daemon-reload

# Restart with new version
sudo systemctl restart novai-validator

# Verify
sudo journalctl -u novai-validator -f
curl http://localhost:8080/health
```

### 9.3 Binary Upgrade

```bash
# Build new version
cd novai-node
git fetch --tags
git checkout mainnet-v1.1.0
git verify-tag mainnet-v1.1.0
cargo build --release

# Stop node
sudo systemctl stop novai-validator

# Replace binary
sudo cp target/release/novai-node /usr/local/bin/novai-node.new
sudo mv /usr/local/bin/novai-node /usr/local/bin/novai-node.old
sudo mv /usr/local/bin/novai-node.new /usr/local/bin/novai-node

# Start node
sudo systemctl start novai-validator

# Verify
sudo journalctl -u novai-validator -f
curl http://localhost:8080/health

# Cleanup (after confirming stable)
sudo rm /usr/local/bin/novai-node.old
```

### 9.4 Rollback Procedure

If upgrade fails:

```bash
# Stop node
sudo systemctl stop novai-validator

# Rollback binary
sudo mv /usr/local/bin/novai-node.old /usr/local/bin/novai-node

# Or rollback Docker
sudo sed -i 's/mainnet-v1.1.0/mainnet-v1.0.0/' /etc/systemd/system/novai-validator.service
sudo systemctl daemon-reload

# Restore state if needed
# (see Section 8.3)

# Start node
sudo systemctl start novai-validator
```

---

## 10. Security Hardening

### 10.1 SSH Hardening

```bash
# /etc/ssh/sshd_config
PermitRootLogin no
PasswordAuthentication no
PubkeyAuthentication yes
MaxAuthTries 3
AllowUsers novai

sudo systemctl restart sshd
```

### 10.2 Firewall Rules Summary

```bash
sudo ufw status verbose
# Should show:
# 22/tcp    ALLOW IN    (SSH)
# 9090/tcp  ALLOW IN    (NOVAI P2P)
# 8080/tcp  ALLOW IN    127.0.0.1 (Metrics - local only)
```

### 10.3 Regular Security Tasks

| Task | Frequency | Command |
|------|-----------|---------|
| System updates | Weekly | `sudo apt update && sudo apt upgrade` |
| Log review | Daily | `sudo journalctl -u novai-validator --since "24 hours ago"` |
| Firewall audit | Monthly | `sudo ufw status verbose` |
| Key access review | Quarterly | Review HSM access logs |

### 10.4 Intrusion Detection

```bash
# Install AIDE (file integrity monitoring)
sudo apt install -y aide
sudo aideinit
sudo mv /var/lib/aide/aide.db.new /var/lib/aide/aide.db

# Run integrity check
sudo aide --check
```

---

## 11. Troubleshooting

### 11.1 Node Not Starting

```bash
# Check service status
sudo systemctl status novai-validator

# Check logs
sudo journalctl -u novai-validator --no-pager -n 100

# Common issues:
# - Port already in use: sudo lsof -i :9090
# - Permission denied: check file ownership
# - Config error: validate config file
```

### 11.2 Node Not Syncing

```bash
# Check peer count
curl -s http://localhost:8080/metrics | grep peer_count

# Check if receiving proposals
sudo journalctl -u novai-validator | grep -i proposal

# Verify network connectivity
telnet seed-1.mainnet.novai.io 9090
```

### 11.3 High Resource Usage

```bash
# Check CPU/memory
htop

# Check disk I/O
iotop

# Check disk space
df -h /var/lib/novai
```

### 11.4 Getting Help

- **Documentation**: `docs/OPERATOR_RUNBOOK.md`
- **GitHub Issues**: https://github.com/novai-protocol/novai-node/issues
- **Discord**: #validator-support channel
- **Emergency**: Contact ceremony coordinator

---

## Appendix A: Quick Reference

### Essential Commands

```bash
# Service management
sudo systemctl start novai-validator
sudo systemctl stop novai-validator
sudo systemctl restart novai-validator
sudo systemctl status novai-validator

# View logs
sudo journalctl -u novai-validator -f
sudo journalctl -u novai-validator --since "1 hour ago"

# Check health
curl http://localhost:8080/health
curl http://localhost:8080/metrics

# Backup
/usr/local/bin/novai-backup.sh
```

### Important Paths

| Path | Purpose |
|------|---------|
| `/var/lib/novai/` | State data |
| `/etc/novai/config/` | Configuration |
| `/etc/novai/keys/` | Validator keys |
| `/etc/novai/genesis/` | Genesis files |
| `/var/log/novai/` | Log files (if configured) |
| `/var/backups/novai/` | Backups |

### Network Endpoints

| Endpoint | Purpose |
|----------|---------|
| `seed-1.mainnet.novai.io:9090` | Seed node 1 |
| `seed-2.mainnet.novai.io:9090` | Seed node 2 |
| `seed-3.mainnet.novai.io:9090` | Seed node 3 |
| `rpc.mainnet.novai.io:8080` | Public RPC |

---

**End of Validator Kit**
