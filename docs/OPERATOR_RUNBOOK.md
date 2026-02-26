# NOVAI Node - Operator Runbook

**Version:** 0.1.0
**Last Updated:** 2026-01-17

## Table of Contents

1. [Quick Start](#1-quick-start)
2. [Common Operations](#2-common-operations)
3. [Troubleshooting](#3-troubleshooting)
4. [Monitoring](#4-monitoring)
5. [Network Configuration](#5-network-configuration)
6. [Data Management](#6-data-management)
7. [Quick Reference](#7-quick-reference)
8. [Security](#8-security)

---

## 1. Quick Start

### Prerequisites

- Docker installed and running
- `novai-node:latest` Docker image built
- Ports 9090-9094 and 8080-8084 available
- Genesis file at `testnet/genesis.json`

### Deploy Full Testnet (5 Validators)

```bash
# Build Docker image
docker build -t novai-node:latest .

# Deploy all 5 validators
./scripts/deploy-testnet.sh
```

**Expected output:**
```
═══════════════════════════════════════════════════
NOVAI Testnet Deployment
═══════════════════════════════════════════════════

Configuration:
  Environment:    local
  Testnet Size:   5 validators
  Seed Node:      validator-0

✅ Prerequisites verified
✅ Deploying seed node (validator 0)...
   Validator 0: Started (abc123def456)
   Validator 0: Ready

✅ All validators deployed

═══════════════════════════════════════════════════
Testnet Status
═══════════════════════════════════════════════════

VALIDATOR       STATUS          CONTAINER IP    P2P      METRICS
---------       ------          ------------    ---      -------
validator-0     running         172.28.0.10     9090     8080
validator-1     running         172.28.0.11     9091     8081
validator-2     running         172.28.0.12     9092     8082
validator-3     running         172.28.0.13     9093     8083
validator-4     running         172.28.0.14     9094     8084

Summary: 5 running, 0 stopped

✅ All validators are running!

Useful commands:
  View logs:        docker logs -f novai-validator-0
  Stop testnet:     ./scripts/cleanup.sh
```

### Deploy Single Validator

```bash
# Deploy validator 0 (seed node, no peers)
./scripts/deploy-validator.sh --validator-id 0

# Deploy validator 1, connecting to validator 0
./scripts/deploy-validator.sh --validator-id 1 --peer 172.28.0.10:9090
```

### Verify Node is Working

**1. Check health endpoint:**
```bash
curl http://localhost:8080/health
```
Expected: `OK`

**2. Check metrics endpoint:**
```bash
curl http://localhost:8080/metrics
```
Expected output:
```
# HELP novai_committed_height Height of last committed block
# TYPE novai_committed_height gauge
novai_committed_height 0

# HELP novai_current_round Current consensus round
# TYPE novai_current_round gauge
novai_current_round 0

# HELP novai_peer_count Number of connected peers
# TYPE novai_peer_count gauge
novai_peer_count 4

# HELP novai_mempool_size Transactions pending in mempool
# TYPE novai_mempool_size gauge
novai_mempool_size 0

# HELP novai_consensus_view_changes_total Total view changes (round advances)
# TYPE novai_consensus_view_changes_total counter
novai_consensus_view_changes_total 0
```

**3. Check peer connections in logs:**
```bash
docker logs novai-validator-0 | grep -E "(Connected|peer)"
```
Expected:
```
✅ Connected to peer 172.28.0.11:9091
✅ Connected to peer 172.28.0.12:9092
✅ Connected to peer 172.28.0.13:9093
✅ Connected to peer 172.28.0.14:9094
   Connected peers: 4
```

**4. Check container status:**
```bash
docker ps | grep novai-validator
```
Expected: All containers with `Up` status.

### Expected Startup Logs

**Seed node (validator 0):**
```
🚀 Starting consensus node
   Port: 9090
   Validator index: 0
   Address: [0, 0, 0, 0, 0, 0, 0, 0]
   Peers: []
📊 Metrics server listening on http://0.0.0.0:8080
✅ Node started, waiting for peers...
👂 Listening for proposals...
   Connected peers: 0
```

**Non-seed node (validator 1):**
```
🚀 Starting consensus node
   Port: 9091
   Validator index: 1
   Address: [1, 1, 1, 1, 1, 1, 1, 1]
   Peers: ["172.28.0.10:9090"]
📊 Metrics server listening on http://0.0.0.0:8080
✅ Connected to peer 172.28.0.10:9090
✅ Node started, waiting for peers...
👂 Listening for proposals...
   Connected peers: 1
```

### First-Time Setup Checklist

- [ ] Docker installed and running
- [ ] Project cloned: `git clone <repo>`
- [ ] Docker image built: `docker build -t novai-node:latest .`
- [ ] Genesis file exists: `testnet/genesis.json`
- [ ] Deployment scripts executable: `chmod +x scripts/*.sh`
- [ ] Ports available: `9090-9094`, `8080-8084`
- [ ] Testnet deployed: `./scripts/deploy-testnet.sh`
- [ ] All validators running: `docker ps | grep novai-validator`
- [ ] Metrics accessible: `curl http://localhost:8080/metrics`
- [ ] Logs show peer connections

---

## 2. Common Operations

### Start Validator

**Single validator:**
```bash
./scripts/deploy-validator.sh --validator-id 0
```

**With peer connections:**
```bash
./scripts/deploy-validator.sh --validator-id 2 --peer 172.28.0.10:9090
```

**Full testnet:**
```bash
./scripts/deploy-testnet.sh
```

**Clean start (remove existing container/volume):**
```bash
./scripts/deploy-validator.sh --validator-id 1 --clean --peer 172.28.0.10:9090
```

### Stop Validator

**Stop specific validator:**
```bash
docker stop novai-validator-0
```

**Stop with graceful timeout (30 seconds):**
```bash
docker stop -t 30 novai-validator-0
```

**Stop all validators:**
```bash
docker stop novai-validator-0 novai-validator-1 novai-validator-2 novai-validator-3 novai-validator-4
```

**Using cleanup script (keeps data):**
```bash
./scripts/cleanup.sh --validator-id 0
```

### Restart Validator

**Method 1: Docker restart**
```bash
docker restart novai-validator-0
```

**Method 2: Stop and start existing container**
```bash
docker stop novai-validator-0
docker start novai-validator-0
```

**Method 3: Full redeploy (using cleanup script)**
```bash
./scripts/cleanup.sh --validator-id 0
./scripts/deploy-validator.sh --validator-id 0
```

### View Logs

**Follow live logs:**
```bash
docker logs -f novai-validator-0
```

**View last 100 lines:**
```bash
docker logs --tail 100 novai-validator-0
```

**Filter for errors:**
```bash
docker logs novai-validator-0 | grep "❌"
```

**Filter for proposals:**
```bash
docker logs novai-validator-0 | grep -E "(Proposing|Proposal)"
```

**Filter for consensus events:**
```bash
docker logs novai-validator-0 | grep -E "(QC|COMMIT|ROUND)"
```

**Search for specific block height:**
```bash
docker logs novai-validator-0 | grep "height=42"
```

**View logs with timestamps:**
```bash
docker logs -t novai-validator-0
```

### Check Sync Status

**Query committed height from metrics:**
```bash
curl -s http://localhost:8080/metrics | grep novai_committed_height
```

**Compare all validators:**
```bash
for port in {8080..8084}; do
  echo "Validator $((port-8080)): $(curl -s http://localhost:$port/metrics | grep committed_height | awk '{print $2}')"
done
```

Expected output:
```
Validator 0: 42
Validator 1: 42
Validator 2: 42
Validator 3: 41  ← Behind by 1 block
Validator 4: 42
```

**Check if validator is catching up (look for sync logs):**
```bash
docker logs novai-validator-3 | grep -E "(RECOVERY|catch-up|sync)"
```

Expected when catching up:
```
🔄 RECOVERY mode: committed_height=40, peer_committed=42
📥 Requesting blocks 41-42 from peer...
✅ Applied block 41, new committed_height=41
✅ Applied block 42, new committed_height=42
✅ Caught up to committed_height=42
```

### Upgrade Procedure

**1. Build new Docker image:**
```bash
docker build \
  --build-arg VERSION=0.2.0 \
  --build-arg GIT_COMMIT=$(git rev-parse --short HEAD) \
  -t novai-node:0.2.0 \
  -t novai-node:latest \
  .
```

**2. Stop validator gracefully (30 second timeout):**
```bash
docker stop -t 30 novai-validator-0
```

**3. Remove old container (keeps volume):**
```bash
docker rm novai-validator-0
```

**4. Start with new image:**
```bash
./scripts/deploy-validator.sh --validator-id 0
```

**5. Verify sync resumes:**
```bash
# Check committed_height is advancing
watch -n 1 'curl -s http://localhost:8080/metrics | grep committed_height'

# Check logs show recovery
docker logs -f novai-validator-0 | grep -E "(RECOVERED|committed_height)"
```

Expected:
```
🔄 RECOVERED consensus state: committed_height=42, highest_qc=Some(42)
✅ Node started at height=43
```

**Rolling upgrade (no downtime for testnet):**
```bash
# Upgrade validators one at a time, wait for sync between each
for i in 1 2 3 4; do
  echo "Upgrading validator $i..."
  docker stop -t 30 novai-validator-$i
  docker rm novai-validator-$i
  ./scripts/deploy-validator.sh --validator-id $i --peer 172.28.0.10:9090

  # Wait for catch-up (check committed_height matches validator 0)
  sleep 10
done

# Finally upgrade validator 0 (seed)
docker stop -t 30 novai-validator-0
docker rm novai-validator-0
./scripts/deploy-validator.sh --validator-id 0
```

---

## 3. Troubleshooting

### Validator Not Connecting to Peers

**Symptoms:**
- `peer_count` metric is 0 or lower than expected
- Logs show "⚠️ Failed to connect to <peer>"
- Container is running but isolated

**Diagnostics:**

**1. Check Docker network:**
```bash
docker network inspect novai-testnet
```

Verify all validators are attached to the network and have correct IPs:
```json
"Containers": {
    "abc123": {
        "Name": "novai-validator-0",
        "IPv4Address": "172.28.0.10/16"
    },
    "def456": {
        "Name": "novai-validator-1",
        "IPv4Address": "172.28.0.11/16"
    }
}
```

**2. Verify ports are available:**
```bash
# Check if ports are listening
netstat -an | grep -E "909[0-4]"

# Should show:
tcp        0      0 0.0.0.0:9090            0.0.0.0:*               LISTEN
tcp        0      0 0.0.0.0:9091            0.0.0.0:*               LISTEN
```

**3. Check firewall rules:**
```bash
# macOS
sudo pfctl -sr | grep 9090

# Linux
sudo iptables -L -n | grep 9090
```

**4. Verify peer addresses in logs:**
```bash
docker logs novai-validator-1 | grep -i peer
```

Expected:
```
Peers: ["172.28.0.10:9090"]
✅ Connected to peer 172.28.0.10:9090
```

**5. Test connectivity manually:**
```bash
# From host
telnet 172.28.0.10 9090

# From another container
docker exec novai-validator-1 sh -c 'nc -zv 172.28.0.10 9090'
```

**Solutions:**

- **Network not created:** Run `docker network create --subnet=172.28.0.0/16 novai-testnet`
- **Port conflict:** Check `lsof -i :9090` and kill conflicting process
- **Wrong peer address:** Verify peer IP in deployment command
- **Container not on network:** Remove and recreate with correct `--network` flag
- **Firewall blocking:** Allow ports 9090-9094 in firewall rules

### Node Falling Behind

**Symptoms:**
- `committed_height` metric is lower than other validators
- Logs show no new commits
- Validator appears "stuck" at old height

**Diagnostics:**

**1. Check committed height across all validators:**
```bash
for port in {8080..8084}; do
  height=$(curl -s http://localhost:$port/metrics | grep committed_height | awk '{print $2}')
  echo "Validator $((port-8080)): height=$height"
done
```

**2. Check peer count:**
```bash
curl -s http://localhost:8081/metrics | grep peer_count
```
Expected: `peer_count >= 3` (need 3 for BFT quorum in 5-validator set)

**3. Check view changes (timeouts):**
```bash
curl -s http://localhost:8081/metrics | grep view_changes_total
```
High value (>10) indicates frequent timeouts.

**4. Check logs for sync activity:**
```bash
docker logs novai-validator-1 | grep -E "(catch-up|RECOVERY|sync)"
```

**5. Check if validator is receiving proposals:**
```bash
docker logs novai-validator-1 | grep "Received proposal"
```

**Solutions:**

- **Restart to trigger catch-up:**
  ```bash
  docker restart novai-validator-1
  # Watch logs for recovery
  docker logs -f novai-validator-1 | grep -E "(RECOVERED|catch-up)"
  ```

- **Check peer_count < 3:** Add more peer connections or check network connectivity

- **High view_changes:** Network partition or leader is down, check all validators running

- **Data corruption:** Full wipe and resync:
  ```bash
  ./scripts/cleanup.sh --validator-id 1 --all
  ./scripts/deploy-validator.sh --validator-id 1 --peer 172.28.0.10:9090
  ```

### High Memory Usage

**Symptoms:**
- Container using > 1GB RAM
- Docker stats shows high memory
- System becomes slow

**Diagnostics:**

**1. Check container memory:**
```bash
docker stats --no-stream | grep novai-validator
```

**2. Check mempool size:**
```bash
curl -s http://localhost:8080/metrics | grep mempool_size
```
Default cap: 1000 transactions

**3. Check for memory leak in logs:**
```bash
docker logs novai-validator-0 | grep -i "memory\|oom"
```

**Solutions:**

- **Large mempool:** Restart to clear:
  ```bash
  docker restart novai-validator-0
  ```

- **Memory leak:** Upgrade to newer version

- **Limit container memory:**
  ```bash
  docker update --memory 512m --memory-swap 512m novai-validator-0
  ```

### Consensus Stuck

**Symptoms:**
- `current_round` > 5 and increasing
- `committed_height` not advancing
- `view_changes_total` increasing rapidly
- Logs show repeated timeouts

**Diagnostics:**

**1. Check current round:**
```bash
curl -s http://localhost:8080/metrics | grep current_round
```
Normal: 0-1. Warning: 2-4. Critical: 5+

**2. Check view changes rate:**
```bash
# Take two samples 10 seconds apart
before=$(curl -s http://localhost:8080/metrics | grep view_changes_total | awk '{print $2}')
sleep 10
after=$(curl -s http://localhost:8080/metrics | grep view_changes_total | awk '{print $2}')
echo "View changes per 10s: $((after - before))"
```

**3. Check peer count:**
```bash
curl -s http://localhost:8080/metrics | grep peer_count
```
Need >= 3 for quorum (in 5-validator set, f=1, quorum=2f+1=3)

**4. Identify leader:**
```bash
# Leader rotates: leader_id = (height + round) % 5
# If height=10, round=3 → leader=(10+3)%5=3 → validator-3

# Check if leader container is running
docker ps | grep novai-validator-3
```

**5. Check for network partition:**
```bash
# Check if validators can reach each other
for i in {0..4}; do
  echo "Validator $i peer_count: $(curl -s http://localhost:808$i/metrics | grep peer_count | awk '{print $2}')"
done
```

Expected: All should have peer_count=4. If split (e.g., 2 and 2), network partition exists.

**6. Check round timeout progression:**
```bash
docker logs novai-validator-0 | grep "ROUND ADVANCED"
```

Expected exponential backoff:
```
⏰ ROUND ADVANCED to round=1 at height=11 (received 3 timeouts)  # 2s timeout
⏰ ROUND ADVANCED to round=2 at height=11 (received 3 timeouts)  # 4s timeout
⏰ ROUND ADVANCED to round=3 at height=11 (received 3 timeouts)  # 8s timeout
⏰ ROUND ADVANCED to round=4 at height=11 (received 3 timeouts)  # 16s timeout
⏰ ROUND ADVANCED to round=5 at height=11 (received 3 timeouts)  # 32s timeout
⏰ ROUND ADVANCED to round=6 at height=11 (received 3 timeouts)  # 60s timeout (capped)
```

**Solutions:**

- **Peer count < 3:** Network partition or validators offline
  ```bash
  # Check all validators running
  docker ps | grep novai-validator

  # Restart validators to reconnect
  for i in {1..4}; do docker restart novai-validator-$i; done
  ```

- **Leader offline:** Check and restart leader container
  ```bash
  docker restart novai-validator-3
  ```

- **Network partition:** Check Docker network connectivity:
  ```bash
  # Recreate network (stops all validators)
  ./scripts/cleanup.sh
  ./scripts/deploy-testnet.sh
  ```

- **Stuck in high round:** Wait for timeout cap (60s) or restart all validators:
  ```bash
  for i in {0..4}; do docker restart novai-validator-$i; done
  ```

### Container Exiting Immediately

**Symptoms:**
- `docker ps` shows no validator containers
- Container exits right after start

**Diagnostics:**

```bash
# Check container exit code
docker ps -a | grep novai-validator-0

# View logs from stopped container
docker logs novai-validator-0

# Check last exit status
docker inspect novai-validator-0 | grep -A 5 "State"
```

**Common causes and solutions:**

- **Port already in use:**
  ```
  Error: Address already in use
  ```
  Solution: Kill process using port or change port in deployment script

- **Invalid --validator index:**
  ```
  Error: validator index must be 0-4
  ```
  Solution: Use valid index (0-4)

- **Missing genesis file:**
  ```
  Error: genesis file not found
  ```
  Solution: Ensure `testnet/genesis.json` exists

- **Permission denied on /data:**
  ```
  Error: permission denied: /data
  ```
  Solution: Fix volume permissions or recreate volume

---

## 4. Monitoring

### Metrics Endpoint

**Access metrics:**
```bash
curl http://localhost:8080/metrics
```

**Metrics exposed:**

| Metric | Type | Description | Normal Range |
|--------|------|-------------|--------------|
| `novai_committed_height` | gauge | Height of last committed block | 0 → ∞ (increasing) |
| `novai_current_round` | gauge | Current consensus round | 0-1 (normal), 2-4 (warning), 5+ (critical) |
| `novai_peer_count` | gauge | Number of connected peers | 4 (in 5-validator testnet) |
| `novai_mempool_size` | gauge | Transactions pending in mempool | 0-1000 (cap=1000) |
| `novai_consensus_view_changes_total` | counter | Total view changes (round advances) | Low (< 10), increasing slowly |

**Query specific metric:**
```bash
# Committed height
curl -s http://localhost:8080/metrics | grep committed_height | awk '{print $2}'

# Peer count
curl -s http://localhost:8080/metrics | grep peer_count | awk '{print $2}'

# View changes rate (last 60s)
watch -n 1 'curl -s http://localhost:8080/metrics | grep view_changes_total'
```

**Compare metrics across all validators:**
```bash
#!/bin/bash
echo "VALIDATOR | HEIGHT | ROUND | PEERS | MEMPOOL | VIEW_CHANGES"
echo "----------|--------|-------|-------|---------|-------------"
for i in {0..4}; do
  port=$((8080 + i))
  metrics=$(curl -s http://localhost:$port/metrics)
  height=$(echo "$metrics" | grep committed_height | awk '{print $2}')
  round=$(echo "$metrics" | grep current_round | awk '{print $2}')
  peers=$(echo "$metrics" | grep peer_count | awk '{print $2}')
  mempool=$(echo "$metrics" | grep mempool_size | awk '{print $2}')
  views=$(echo "$metrics" | grep view_changes_total | awk '{print $2}')
  printf "%-9s | %-6s | %-5s | %-5s | %-7s | %-12s\n" "$i" "$height" "$round" "$peers" "$mempool" "$views"
done
```

### Prometheus Configuration

**Install Prometheus:**
```bash
# macOS
brew install prometheus

# Linux
sudo apt-get install prometheus

# Docker
docker run -d -p 9090:9090 -v $(pwd)/prometheus.yml:/etc/prometheus/prometheus.yml prom/prometheus
```

**Configure scraping (`prometheus.yml`):**
```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'novai-validators'
    static_configs:
      - targets:
          - 'localhost:8080'  # validator-0
          - 'localhost:8081'  # validator-1
          - 'localhost:8082'  # validator-2
          - 'localhost:8083'  # validator-3
          - 'localhost:8084'  # validator-4
        labels:
          network: 'testnet'

  - job_name: 'novai-metrics'
    static_configs:
      - targets: ['localhost:8080']
        labels:
          validator_id: '0'
      - targets: ['localhost:8081']
        labels:
          validator_id: '1'
      - targets: ['localhost:8082']
        labels:
          validator_id: '2'
      - targets: ['localhost:8083']
        labels:
          validator_id: '3'
      - targets: ['localhost:8084']
        labels:
          validator_id: '4'
```

**Start Prometheus:**
```bash
prometheus --config.file=prometheus.yml
```

**Access Prometheus UI:**
```
http://localhost:9090
```

**Example queries:**
```promql
# Current committed height across all validators
novai_committed_height

# Validators with peer_count < 3 (critical)
novai_peer_count < 3

# View changes rate (per minute)
rate(novai_consensus_view_changes_total[1m])

# Height difference between validators (detect lag)
max(novai_committed_height) - min(novai_committed_height)

# Validators stuck in high rounds
novai_current_round > 5
```

### Grafana Dashboard

**Import dashboard:**

1. Open Grafana: `http://localhost:3000`
2. Navigate to: **Dashboards → Import**
3. Upload file: `dashboards/novai-grafana.json` *(not yet included in repo — create from Prometheus metrics below)*
4. Select Prometheus datasource
5. Click **Import**

**Dashboard panels:**

- **Committed Height** (stat) - Current chain tip
- **Current Round** (stat) - Consensus round (yellow if >2, red if >5)
- **Connected Peers** (stat) - Peer count (red if <3)
- **Mempool Size** (stat) - Pending transactions (yellow if >500, red if >800)
- **Block Height Over Time** (graph) - Block production rate
- **View Changes Rate** (graph) - Timeout frequency (per minute)
- **Peer Count Over Time** (graph) - Network stability
- **Mempool Size Over Time** (graph) - Transaction demand

**Refresh rate:** 5 seconds

**Time range:** Last 30 minutes (adjustable)

### Alert Thresholds

**Configure Prometheus alerts (`alerts.yml`):**
```yaml
groups:
  - name: novai_alerts
    interval: 30s
    rules:
      # CRITICAL: Committed height stalled
      - alert: ConsensusStalled
        expr: rate(novai_committed_height[60s]) == 0
        for: 60s
        labels:
          severity: critical
        annotations:
          summary: "Consensus stalled on validator {{ $labels.instance }}"
          description: "No new blocks committed in the last 60 seconds."

      # CRITICAL: No BFT quorum (peer_count < 3 in 5-validator set)
      - alert: InsufficientPeers
        expr: novai_peer_count < 3
        for: 30s
        labels:
          severity: critical
        annotations:
          summary: "Insufficient peers on {{ $labels.instance }}"
          description: "Peer count is {{ $value }}, need >= 3 for BFT quorum."

      # WARNING: Consensus slow (stuck in high rounds)
      - alert: HighConsensusRound
        expr: novai_current_round > 5
        for: 60s
        labels:
          severity: warning
        annotations:
          summary: "High consensus round on {{ $labels.instance }}"
          description: "Current round is {{ $value }}, indicates timeout issues."

      # WARNING: Frequent view changes (timeouts)
      - alert: FrequentViewChanges
        expr: rate(novai_consensus_view_changes_total[5m]) > 10
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Frequent view changes on {{ $labels.instance }}"
          description: "{{ $value }} view changes per second, check network stability."

      # WARNING: Mempool approaching capacity
      - alert: MempoolNearFull
        expr: novai_mempool_size > 800
        for: 60s
        labels:
          severity: warning
        annotations:
          summary: "Mempool near capacity on {{ $labels.instance }}"
          description: "Mempool has {{ $value }} transactions (cap=1000)."

      # CRITICAL: Validator height lagging
      - alert: ValidatorLagging
        expr: max(novai_committed_height) - novai_committed_height > 10
        for: 60s
        labels:
          severity: critical
        annotations:
          summary: "Validator {{ $labels.instance }} is lagging"
          description: "{{ $value }} blocks behind the highest validator."
```

**Alert notification (Alertmanager config):**
```yaml
route:
  receiver: 'slack'
  group_by: ['alertname', 'instance']
  group_wait: 10s
  group_interval: 10s
  repeat_interval: 1h

receivers:
  - name: 'slack'
    slack_configs:
      - api_url: 'YOUR_SLACK_WEBHOOK_URL'
        channel: '#novai-alerts'
        title: 'NOVAI Alert: {{ .GroupLabels.alertname }}'
        text: '{{ range .Alerts }}{{ .Annotations.description }}{{ end }}'
```

---

## 5. Network Configuration

### Port Assignments

**Default port mapping:**

| Validator | P2P Port (host) | Metrics Port (host) | Container IP |
|-----------|-----------------|---------------------|--------------|
| 0 (seed)  | 9090            | 8080                | 172.28.0.10  |
| 1         | 9091            | 8081                | 172.28.0.11  |
| 2         | 9092            | 8082                | 172.28.0.12  |
| 3         | 9093            | 8083                | 172.28.0.13  |
| 4         | 9094            | 8084                | 172.28.0.14  |

**Port mapping explanation:**
- **P2P Port:** Validator N → `9090 + N`
- **Metrics Port:** Validator N → `8080 + N`
- **Container IP:** Validator N → `172.28.0.10 + N`

**Check port usage:**
```bash
# macOS
lsof -i :9090-9094
lsof -i :8080-8084

# Linux
netstat -tulpn | grep -E "(909[0-4]|808[0-4])"
```

**Override ports in deployment:**
```bash
# Deploy validator 5 on custom ports (if extending beyond 5 validators)
docker run -d \
  --name novai-validator-5 \
  --network novai-testnet \
  -p 9095:9090 \
  -p 8085:8080 \
  novai-node:latest \
  run --port 9090 --validator 5 --peer 172.28.0.10:9090
```

### Docker Network

**Network name:** `novai-testnet`
**Subnet:** `172.28.0.0/16`
**Driver:** bridge

**Create network manually:**
```bash
docker network create \
  --driver bridge \
  --subnet 172.28.0.0/16 \
  novai-testnet
```

**Inspect network:**
```bash
docker network inspect novai-testnet
```

**List containers on network:**
```bash
docker network inspect novai-testnet | jq -r '.Containers[] | "\(.Name): \(.IPv4Address)"'
```

Expected:
```
novai-validator-0: 172.28.0.10/16
novai-validator-1: 172.28.0.11/16
novai-validator-2: 172.28.0.12/16
novai-validator-3: 172.28.0.13/16
novai-validator-4: 172.28.0.14/16
```

**Remove network (after stopping all containers):**
```bash
docker network rm novai-testnet
```

### Network Topology

**Star topology with seed node:**

```
                    172.28.0.10:9090
                    ┌─────────────┐
                    │ validator-0 │ ← Seed node
                    │   (seed)    │
                    └──────┬──────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
   172.28.0.11        172.28.0.12        172.28.0.13        172.28.0.14
   ┌──────────┐       ┌──────────┐       ┌──────────┐       ┌──────────┐
   │validator-1│       │validator-2│       │validator-3│       │validator-4│
   │  (peer)   │       │  (peer)   │       │  (peer)   │       │  (peer)   │
   └──────────┘       └──────────┘       └──────────┘       └──────────┘
```

**Peer configuration:**
- **Validator 0:** No peers specified (seed node, listens for incoming connections)
- **Validators 1-4:** Connect to validator 0 at `172.28.0.10:9090`

**After initial connection:**
- All validators discover each other via gossip
- Full mesh connectivity is established
- Any validator can propose/vote

**Verify full mesh connectivity:**
```bash
# All validators should have peer_count=4
for i in {0..4}; do
  count=$(curl -s http://localhost:808$i/metrics | grep peer_count | awk '{print $2}')
  echo "Validator $i: $count peers"
done
```

Expected:
```
Validator 0: 4 peers
Validator 1: 4 peers
Validator 2: 4 peers
Validator 3: 4 peers
Validator 4: 4 peers
```

### Firewall Configuration

**Allow P2P and metrics ports:**

**macOS (pf):**
```bash
# Add to /etc/pf.conf
pass in proto tcp from any to any port 9090:9094
pass in proto tcp from any to any port 8080:8084

# Reload
sudo pfctl -f /etc/pf.conf
```

**Linux (iptables):**
```bash
# Allow P2P ports
sudo iptables -A INPUT -p tcp --dport 9090:9094 -j ACCEPT

# Allow metrics ports (restrict to localhost if sensitive)
sudo iptables -A INPUT -p tcp -s 127.0.0.1 --dport 8080:8084 -j ACCEPT

# Save rules
sudo iptables-save > /etc/iptables/rules.v4
```

**Linux (ufw):**
```bash
# Allow P2P ports
sudo ufw allow 9090:9094/tcp

# Allow metrics ports from localhost only
sudo ufw allow from 127.0.0.1 to any port 8080:8084 proto tcp
```

---

## 6. Data Management

### Volume Locations

**Named volumes (default):**
```
novai-validator-0-data
novai-validator-1-data
novai-validator-2-data
novai-validator-3-data
novai-validator-4-data
```

**List volumes:**
```bash
docker volume ls | grep novai-validator
```

**Inspect volume:**
```bash
docker volume inspect novai-validator-0-data
```

Expected output:
```json
[
    {
        "Driver": "local",
        "Labels": {},
        "Mountpoint": "/var/lib/docker/volumes/novai-validator-0-data/_data",
        "Name": "novai-validator-0-data",
        "Options": {},
        "Scope": "local"
    }
]
```

**Volume contents:**
```
/data/
├── blocks/           # Persisted blocks (via KV store)
├── state/            # State commitments (via KV store)
└── committed_height  # Highest committed block
```

**Access volume data:**
```bash
# Run shell in temporary container with volume mounted
docker run --rm -it -v novai-validator-0-data:/data alpine sh

# Inside container
ls -la /data
```

### Backup Procedure

**1. Stop validator (to ensure consistent state):**
```bash
docker stop novai-validator-0
```

**2. Backup volume to tar.gz:**
```bash
docker run --rm \
  -v novai-validator-0-data:/data \
  -v $(pwd):/backup \
  alpine \
  tar czf /backup/novai-validator-0-backup-$(date +%Y%m%d-%H%M%S).tar.gz /data
```

**3. Restart validator:**
```bash
docker start novai-validator-0
```

**4. Verify backup:**
```bash
tar -tzf novai-validator-0-backup-*.tar.gz | head
```

**Backup all validators:**
```bash
#!/bin/bash
BACKUP_DIR="./backups/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"

for i in {0..4}; do
  echo "Backing up validator $i..."
  docker stop novai-validator-$i

  docker run --rm \
    -v novai-validator-$i-data:/data \
    -v "$BACKUP_DIR":/backup \
    alpine \
    tar czf /backup/validator-$i.tar.gz /data

  docker start novai-validator-$i
done

echo "Backups saved to: $BACKUP_DIR"
```

### Restore from Backup

**1. Stop validator:**
```bash
docker stop novai-validator-0
docker rm novai-validator-0
```

**2. Remove existing volume:**
```bash
docker volume rm novai-validator-0-data
```

**3. Create new volume:**
```bash
docker volume create novai-validator-0-data
```

**4. Restore from backup:**
```bash
docker run --rm \
  -v novai-validator-0-data:/data \
  -v $(pwd):/backup \
  alpine \
  tar xzf /backup/novai-validator-0-backup-20260117-143022.tar.gz -C /
```

**5. Restart validator:**
```bash
./scripts/deploy-validator.sh --validator-id 0
```

**6. Verify recovery:**
```bash
docker logs novai-validator-0 | grep RECOVERED
```

Expected:
```
🔄 RECOVERED consensus state: committed_height=42, highest_qc=Some(42)
```

### Clean Restart (Keep Data)

**Single validator:**
```bash
# Remove container, keep volume
./scripts/cleanup.sh --validator-id 0

# Redeploy (reuses existing volume)
./scripts/deploy-validator.sh --validator-id 0
```

**All validators:**
```bash
# Remove containers, keep volumes and network
./scripts/cleanup.sh

# Redeploy
./scripts/deploy-testnet.sh
```

**Verify data persisted:**
```bash
# Check committed_height matches pre-restart value
curl -s http://localhost:8080/metrics | grep committed_height
```

### Full Wipe (Delete All Data)

**⚠️ WARNING: This deletes ALL blockchain data. Cannot be undone.**

**Single validator:**
```bash
./scripts/cleanup.sh --validator-id 0 --all
```

**All validators:**
```bash
./scripts/cleanup.sh --all
```

**Manual full wipe:**
```bash
# Stop all validators
docker stop novai-validator-{0..4}

# Remove containers
docker rm novai-validator-{0..4}

# Remove volumes
docker volume rm novai-validator-{0..4}-data

# Remove network
docker network rm novai-testnet
```

**Redeploy fresh testnet:**
```bash
./scripts/deploy-testnet.sh
```

**Verify fresh state:**
```bash
# committed_height should be 0
curl -s http://localhost:8080/metrics | grep committed_height
```

Expected: `novai_committed_height 0`

### Disk Space Management

**Check volume sizes:**
```bash
docker system df -v | grep novai-validator
```

**Check total disk usage:**
```bash
du -sh /var/lib/docker/volumes/novai-validator-*
```

**Prune unused volumes (be careful):**
```bash
# Remove volumes not attached to containers
docker volume prune -f
```

**Archive old backups:**
```bash
# Compress backups older than 7 days
find ./backups -name "*.tar.gz" -mtime +7 -exec gzip {} \;

# Delete backups older than 30 days
find ./backups -name "*.tar.gz" -mtime +30 -delete
```

---

## 7. Quick Reference

### Command Cheatsheet

```bash
# Deploy
./scripts/deploy-testnet.sh              # Deploy all 5 validators
./scripts/deploy-validator.sh --validator-id 0  # Deploy single validator

# Status
docker ps | grep novai-validator         # List running validators
docker logs -f novai-validator-0         # Follow logs
curl http://localhost:8080/metrics       # Check metrics
curl http://localhost:8080/health        # Health check

# Control
docker stop novai-validator-0            # Stop validator
docker start novai-validator-0           # Start validator
docker restart novai-validator-0         # Restart validator

# Cleanup
./scripts/cleanup.sh                     # Remove containers (keep data)
./scripts/cleanup.sh --all               # Full wipe (delete data)
./scripts/cleanup.sh --validator-id 0    # Clean specific validator

# Monitoring
for i in {0..4}; do curl -s http://localhost:808$i/metrics | grep committed_height; done
docker stats --no-stream | grep novai-validator

# Logs
docker logs novai-validator-0 | grep "❌"        # Errors
docker logs novai-validator-0 | grep "COMMIT"   # Commits
docker logs novai-validator-0 | grep "QC"       # QC formation

# Backup
docker run --rm -v novai-validator-0-data:/data -v $(pwd):/backup alpine tar czf /backup/backup.tar.gz /data
```

### Port Mapping Table

| Validator | Container Name       | P2P (Host) | Metrics (Host) | Container IP |
|-----------|---------------------|------------|----------------|--------------|
| 0         | novai-validator-0   | 9090       | 8080           | 172.28.0.10  |
| 1         | novai-validator-1   | 9091       | 8081           | 172.28.0.11  |
| 2         | novai-validator-2   | 9092       | 8082           | 172.28.0.12  |
| 3         | novai-validator-3   | 9093       | 8083           | 172.28.0.13  |
| 4         | novai-validator-4   | 9094       | 8084           | 172.28.0.14  |

### Validator Address Table

| Validator ID | Address (First 8 Bytes Hex) | Pubkey Pattern |
|--------------|----------------------------|----------------|
| 0            | 00000000...                | [0, 0, 0, ...] |
| 1            | 01010101...                | [1, 1, 1, ...] |
| 2            | 02020202...                | [2, 2, 2, ...] |
| 3            | 03030303...                | [3, 3, 3, ...] |
| 4            | 04040404...                | [4, 4, 4, ...] |

### Common Log Patterns

| Log Message | Meaning | Action |
|-------------|---------|--------|
| `🚀 Starting consensus node` | Node starting up | Normal |
| `📊 Metrics server listening` | Metrics endpoint ready | Normal |
| `✅ Connected to peer` | Peer connection successful | Normal |
| `👑 We are leader, proposing block` | Node is leader, proposing | Normal |
| `✅ Proposal accepted` | Received valid proposal | Normal |
| `✅ QC FORMED` | Quorum certificate formed | Normal |
| `✅ COMMIT` | Block committed to chain | Normal |
| `⏰ ROUND ADVANCED` | Timeout triggered, round increased | Warning (check why) |
| `🔄 RECOVERED` | Node restarted, recovered state | Normal after restart |
| `❌ Propose failed` | Leader proposal failed | Check logs for details |
| `❌ Timeout broadcast failed` | Network issue | Check peer connectivity |
| `⚠️ Failed to connect to peer` | Peer unreachable | Check peer is running |

### Metrics Normal Ranges

| Metric | Normal | Warning | Critical |
|--------|--------|---------|----------|
| `committed_height` | Increasing steadily | Stalled < 30s | Stalled > 60s |
| `current_round` | 0-1 | 2-4 | 5+ |
| `peer_count` | 4 (in 5-validator set) | 2-3 | < 2 |
| `mempool_size` | 0-500 | 500-800 | 800-1000 |
| `view_changes_total` | Low, slow increase | Rapid increase | Very rapid increase |

---

## 8. Security

### Testnet vs Production

**⚠️ CRITICAL WARNINGS:**

1. **Testnet keys are deterministic and publicly known**
   - Validator 0: `SigningKey::from_bytes(&[0; 32])`
   - Validator 1: `SigningKey::from_bytes(&[1; 32])`
   - ... etc.

2. **DO NOT use these keys in production**
   - Anyone can recreate the private keys
   - No security against key compromise
   - For testnet/devnet ONLY

3. **Production deployment requires:**
   - Secure key generation (`ed25519-dalek` with system entropy)
   - Hardware Security Module (HSM) or secure key storage
   - Key rotation procedures
   - Access control and audit logging

### Validator Key Management

**Testnet keys (hardcoded):**
```rust
// crates/node/src/main.rs
let validator_keys: Vec<SigningKey> = (0..5)
    .map(|i| SigningKey::from_bytes(&[i as u8; 32]))
    .collect();
```

**For production (NOT implemented yet):**
- Store keys in encrypted files
- Use environment variables for key paths
- Implement key rotation without downtime
- Use HSM for signing operations

### Metrics Endpoint Security

**Current state (testnet):**
- Metrics endpoint is **unauthenticated**
- Exposed on `0.0.0.0:8080` (all interfaces)
- Anyone on network can read metrics

**Recommendations for production:**

1. **Bind to localhost only:**
   ```rust
   // Change in main.rs
   metrics::start_metrics_server("127.0.0.1:8080", metrics_collect)
   ```

2. **Use reverse proxy with authentication:**
   ```nginx
   # nginx config
   location /metrics {
       auth_basic "Restricted";
       auth_basic_user_file /etc/nginx/.htpasswd;
       proxy_pass http://127.0.0.1:8080/metrics;
   }
   ```

3. **Firewall restrictions:**
   ```bash
   # Allow only Prometheus server
   sudo iptables -A INPUT -p tcp -s PROMETHEUS_IP --dport 8080 -j ACCEPT
   sudo iptables -A INPUT -p tcp --dport 8080 -j DROP
   ```

4. **TLS encryption:**
   - Terminate TLS at reverse proxy
   - Use client certificates for mutual TLS

### Docker Network Isolation

**Current testnet setup:**
- Custom bridge network `novai-testnet`
- Isolated from default Docker bridge
- Containers can only reach each other

**Production recommendations:**
- Use Docker Swarm or Kubernetes for orchestration
- Network policies to restrict inter-container communication
- Separate networks for P2P and metrics

### Data Protection

**Volume encryption (production):**
```bash
# Use encrypted volumes
docker volume create \
  --driver local \
  --opt type=tmpfs \
  --opt device=tmpfs \
  --opt o=size=10g,uid=65532,encryption=aes-xts-plain64 \
  novai-validator-0-data-encrypted
```

**Backup encryption:**
```bash
# Encrypt backups with GPG
tar czf - /data | gpg --encrypt --recipient you@example.com > backup.tar.gz.gpg

# Decrypt
gpg --decrypt backup.tar.gz.gpg | tar xzf -
```

### Access Control

**Limit Docker access:**
```bash
# Add operator user to docker group (carefully)
sudo usermod -aG docker operator

# Use sudo for privileged operations
# Audit docker commands in logs
```

**Container user (current):**
- Containers run as `nonroot` (UID 65532) in distroless image
- No shell access (distroless has no shell)
- Minimal attack surface

### Audit Logging

**Enable Docker logging:**
```json
// /etc/docker/daemon.json
{
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "10m",
    "max-file": "3",
    "labels": "validator_id"
  }
}
```

**Ship logs to central logging:**
```bash
# Use fluentd or logstash
docker run -d \
  --log-driver=fluentd \
  --log-opt fluentd-address=localhost:24224 \
  --log-opt tag="novai.validator-0" \
  novai-node:latest
```

---

## Appendix: Deployment Scripts Reference

### scripts/deploy-validator.sh

**Purpose:** Deploy single NOVAI validator node

**Usage:**
```bash
./scripts/deploy-validator.sh --validator-id <0-4> [OPTIONS]
```

**Options:**
- `--validator-id <0-4>` - Validator index (required)
- `--environment <env>` - local|aws|digitalocean (default: local)
- `--port <port>` - Override P2P port (default: 9090+id)
- `--peer <addr>` - Peer address (repeatable)
- `--clean` - Remove existing before starting
- `--dry-run` - Preview actions
- `--force` - Skip confirmations
- `--debug` - Enable debug logging
- `--help` - Show help

**Examples:**
```bash
# Deploy validator 0 (seed)
./scripts/deploy-validator.sh --validator-id 0

# Deploy validator 1, connect to seed
./scripts/deploy-validator.sh --validator-id 1 --peer 172.28.0.10:9090

# Clean deploy
./scripts/deploy-validator.sh --validator-id 2 --clean --peer 172.28.0.10:9090
```

### scripts/deploy-testnet.sh

**Purpose:** Deploy full 5-validator NOVAI testnet

**Usage:**
```bash
./scripts/deploy-testnet.sh [OPTIONS]
```

**Options:**
- `--environment <env>` - local|aws|digitalocean (default: local)
- `--clean` - Remove all existing before starting
- `--dry-run` - Preview actions
- `--force` - Skip confirmations
- `--debug` - Enable debug logging
- `--help` - Show help

**Examples:**
```bash
# Deploy testnet
./scripts/deploy-testnet.sh

# Clean deploy
./scripts/deploy-testnet.sh --clean

# Dry run
./scripts/deploy-testnet.sh --dry-run
```

### scripts/cleanup.sh

**Purpose:** Clean up NOVAI validators, volumes, and network

**Usage:**
```bash
./scripts/cleanup.sh [OPTIONS]
```

**Options:**
- `--validator-id <0-4>` - Clean specific validator only
- `--keep-data` - Keep data volumes (default)
- `--keep-network` - Keep Docker network (default)
- `--all` - Remove everything including volumes/network
- `--dry-run` - Preview actions
- `--force` - Skip confirmations
- `--help` - Show help

**Examples:**
```bash
# Remove containers, keep data
./scripts/cleanup.sh

# Full wipe
./scripts/cleanup.sh --all

# Clean specific validator
./scripts/cleanup.sh --validator-id 2
```

---

**End of Operator Runbook**

For issues or questions, refer to:
- Project documentation: `docs/`
- Docker guide: `DOCKER.md`
- Deployment scripts: `scripts/`
- Metrics implementation: `crates/node/src/metrics.rs`
