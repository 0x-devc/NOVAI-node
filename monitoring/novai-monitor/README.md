# novai-monitor

Polls the NOVAI node Prometheus metrics endpoint and fires alerts to
Telegram when chain health degrades. Runs as a long-lived systemd service
alongside `novai-node` on the testnet host. Zero external Python deps
(stdlib only).

## What it watches

| Alert ID | Severity | Window | Triggers when |
|---|---|---|---|
| `block_height_stuck` | CRITICAL | 30s | `novai_committed_height` does not advance |
| `peer_count_below_quorum` | CRITICAL | 60s | `novai_peer_count < 3` (BFT quorum needs 3 of 4) |
| `peer_count_degraded` | WARN | 120s | `novai_peer_count < 4` (any peer missing) |
| `mempool_empty` | WARN | 5m | `novai_mempool_size == 0` (tx flow regression) |
| `mempool_backlog` | WARN | 5m | `novai_mempool_size > 1000` |
| `view_change_spike` | CRITICAL | 3m | view change rate > 6/min |
| `view_change_elevated` | WARN | 5m | view change rate > 2/min |
| `anomaly_published` | WARN | per scrape | new on-chain anomaly signal |
| `anomaly_high_confidence` | CRITICAL | per scrape | `novai_anomaly_last_confidence > 204` (~0.8) |
| `copilot_heartbeat_dead` | WARN | 10m | `novai_copilot_observations_total` flat |
| `proposer_skipping_txs` | WARN | 2m | empty blocks while mempool non-empty |
| `metrics_endpoint_unreachable` | WARN | 2m | scrape failures sustained |

Alerts that map to an existing playbook in `docs/playbooks/` include the
file path in the Telegram message body so the operator can pull up the
runbook on phone.

## One-time Telegram bot setup

1. Open Telegram, search `@BotFather`, send `/start`.
2. Send `/newbot`. Pick a display name (e.g. `NOVAI Monitor`). Pick a
   username ending in `bot` (e.g. `novai_mon_alerts_bot`).
3. Save the HTTP API token that BotFather replies with. Format:
   `123456789:AAH...`. Treat this as a secret.
4. Get your numeric chat ID. Easiest path: search `@userinfobot`, send
   `/start`, it replies with your numeric ID. Alternative: send your
   new bot any message, then `curl
   "https://api.telegram.org/bot<TOKEN>/getUpdates"` and read
   `result[0].message.chat.id`.
5. Send your new bot `/start` once from your account. Bots cannot DM
   accounts that have never messaged them first.
6. Smoke test:
   ```bash
   curl -s "https://api.telegram.org/bot<TOKEN>/sendMessage" \
        -d "chat_id=<ID>" -d "text=hello from BotFather"
   ```
   You should see the message land on your phone within a second.
7. Save the token and chat ID into `/etc/novai/monitor.env` (see
   `systemd/monitor.env.example`).

## Local smoke tests (before deploying)

```bash
cd monitoring/novai-monitor
python3 -m pytest tests/         # all unit tests pass with no deps
```

Run the monitor against a real node in dry-run mode (no Telegram POSTs):

```bash
NOVAI_MONITOR_METRICS_URL=http://localhost:8081/metrics \
NOVAI_MONITOR_LOG_LEVEL=DEBUG \
python3 novai_monitor.py --once --dry-run
```

## Deployment to the testnet host

The script ships as plain files. There is no install package. Copy,
configure, enable.

```bash
# As root on the testnet host:
install -d /opt/novai-monitor /etc/novai /var/lib/novai-monitor
install -m 0644 monitoring/novai-monitor/*.py /opt/novai-monitor/
install -m 0644 monitoring/novai-monitor/systemd/novai-monitor.service \
        /etc/systemd/system/novai-monitor.service
install -m 0600 monitoring/novai-monitor/systemd/monitor.env.example \
        /etc/novai/monitor.env
# Now edit /etc/novai/monitor.env and replace the Telegram REPLACE_ME values.
systemctl daemon-reload
systemctl enable --now novai-monitor
```

Verify it is alive:

```bash
systemctl status novai-monitor
journalctl -u novai-monitor -f -o cat
```

Force a synthetic Telegram message to confirm end-to-end delivery:

```bash
sudo systemctl stop novai-monitor
sudo -E /usr/bin/python3 /opt/novai-monitor/novai_monitor.py --test-alert
sudo systemctl start novai-monitor
```

(The stop/start dance avoids two processes competing for the same
Telegram chat for the duration of the test.)

## Configuration reference

All configuration is via environment variables loaded from
`/etc/novai/monitor.env`. No config file format. Defaults assume
loopback polling of a node listening on `:8081`.

| Variable | Default | Purpose |
|---|---|---|
| `NOVAI_MONITOR_METRICS_URL` | `http://localhost:8081/metrics` | Full URL to scrape |
| `NOVAI_MONITOR_METRICS_USER` | (empty) | Basic auth user (only if URL points off-host) |
| `NOVAI_MONITOR_METRICS_PASS` | (empty) | Basic auth password |
| `NOVAI_MONITOR_POLL_INTERVAL_SECS` | `30` | Base scrape cadence |
| `NOVAI_MONITOR_HTTP_TIMEOUT_SECS` | `10` | Per-scrape HTTP timeout |
| `NOVAI_MONITOR_REARM_GRACE_SECS` | `120` | Startup silence window (still scrapes) |
| `NOVAI_MONITOR_UNREACHABLE_THRESHOLD_SECS` | `120` | When to fire the unreachable alert |
| `NOVAI_MONITOR_TELEGRAM_BOT_TOKEN` | required | From @BotFather |
| `NOVAI_MONITOR_TELEGRAM_CHAT_ID` | required | Numeric chat ID |
| `NOVAI_MONITOR_LOG_LEVEL` | `INFO` | DEBUG / INFO / WARNING / ERROR |
| `NOVAI_MONITOR_ENV_LABEL` | `unknown` | Free-form host label included in every alert |

## Operational notes

- Logging goes to stderr in structured `key=value` form. systemd journal
  captures it. Tail: `journalctl -u novai-monitor -f -o cat`.
- Undelivered alerts (Telegram offline, network partition) are buffered
  at `/var/lib/novai-monitor/alerts_undelivered.jsonl`. Inspect with
  `tail -n 50 /var/lib/novai-monitor/alerts_undelivered.jsonl`. v0 does
  not auto-drain on recovery; the buffer is a forensic trail. Auto-drain
  is a v0.1 follow-up.
- State is in-memory. A restart re-seeds counter baselines and silently
  scrapes for `NOVAI_MONITOR_REARM_GRACE_SECS` before alerting again.
  An ongoing critical incident will re-fire after the grace window.
- The script does not send "I am up" or "I am healthy" pings. The only
  intentional outbound traffic is alert FIRE and RECOVER messages, plus
  the explicit `--test-alert` invocation.

## Known open loops

- v0 monitors a single node. The other testnet validators do not expose
  `/metrics`. Extending to N endpoints is a follow-up when those endpoints
  exist.
- Runs as root in v0. Hardening to a dedicated `novai` Unix user is a
  follow-up; the security gain on a read-only monitor is small relative
  to dropping root on the node itself.
- Undelivered-alert JSONL is write-only in v0; auto-drain on Telegram
  recovery is a v0.1 follow-up. The operator can grep the file manually
  if a delivery outage is suspected.
- State is in-memory; a `state.json` snapshot under
  `/var/lib/novai-monitor/` would survive restart without re-firing
  alerts that were already acknowledged. Deferred until v0 produces
  measurable post-restart noise.
