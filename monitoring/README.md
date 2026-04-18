# NOVAI Monitoring Setup

Prometheus alert rules and Grafana dashboard for NOVAI validator nodes.

## Quick Start

### 1. Prometheus Configuration

Add the NOVAI node as a scrape target in your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'novai'
    scrape_interval: 15s
    static_configs:
      - targets:
          - 'localhost:8081'  # Metrics port (default: --metrics-port 8081)
```

### 2. Alert Rules

Copy `alerts.yml` to your Prometheus rules directory:

```bash
cp monitoring/alerts.yml /etc/prometheus/rules/novai-alerts.yml
```

Add to `prometheus.yml`:

```yaml
rule_files:
  - /etc/prometheus/rules/novai-alerts.yml
```

Reload Prometheus: `kill -HUP $(pgrep prometheus)` or `curl -X POST http://localhost:9090/-/reload`

### 3. Grafana Dashboard

Import `dashboards/novai-grafana.json` via Grafana UI:
1. Go to Dashboards > Import
2. Upload `novai-grafana.json`
3. Select your Prometheus data source
4. Click Import

## Available Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `novai_committed_height` | gauge | Height of last committed block |
| `novai_current_round` | gauge | Current consensus round (high = timeouts) |
| `novai_peer_count` | gauge | Number of connected peers |
| `novai_mempool_size` | gauge | Pending transactions in mempool |
| `novai_consensus_view_changes_total` | counter | Total timeout-triggered round advances |
| `novai_block_tx_count` | gauge | Transactions in last committed block |
| `novai_total_txs_committed` | counter | Cumulative committed transactions |
| `novai_copilot_observations_total` | counter | AI copilot observation cycles |
| `novai_anomaly_signals_total` | counter | Anomaly signals detected |
| `novai_anomaly_signals_published` | counter | Anomaly signals published on-chain |
| `novai_anomaly_last_confidence` | gauge | Last anomaly signal confidence score |

## Alert Rules Summary

### Critical

| Alert | Condition | Description |
|-------|-----------|-------------|
| `ConsensusStalled` | No blocks in 10 min | Consensus completely stopped |
| `ConsensusDelayed` | No blocks in 5 min | Consensus may be stalling |
| `InsufficientPeers` | peer_count < 3 | Cannot form quorum |

### Warning

| Alert | Condition | Description |
|-------|-----------|-------------|
| `HighConsensusRound` | round > 5 | Leader may be failing |
| `FrequentViewChanges` | >10 view changes/min | Network instability |
| `MempoolNearFull` | mempool > 800 txs | Approaching capacity |
| `ValidatorLagging` | >5 blocks behind peers | Node falling behind |
| `HighProposalRate` | >100 proposals/hr | Possible governance spam |

## Endpoints

| Endpoint | URL | Description |
|----------|-----|-------------|
| Metrics | `http://localhost:8081/metrics` | Prometheus text format |
| Health | `http://localhost:8081/health` | Returns "OK" if running |
| RPC | `http://localhost:3030/` | JSON-RPC 2.0 endpoint |

## Troubleshooting

**No metrics appearing in Prometheus:**
- Verify `--metrics-port` is set when starting the node
- Check firewall allows TCP on the metrics port
- Test directly: `curl http://localhost:8081/metrics`

**Alerts not firing:**
- Verify `alerts.yml` is loaded: check Prometheus UI > Status > Rules
- Some alerts use placeholder metrics that fire only after instrumentation
