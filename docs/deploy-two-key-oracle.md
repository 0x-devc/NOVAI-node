# Deploy Notes: Two-Key Type-10 Oracle Migration

One-shot deploy procedure for migrating the production price-oracle
from the single-key Type-8 model to the two-key Type-10 model. The
model is validated locally (80/80 unit tests pass; a live smoke test
on the chain confirmed all four pass criteria). This document is for
the operator to follow on the production box; nothing here was
executed from a Claude Code session.

Reference: `docs/gate-oracle-funding-model-diagnosis.md` (design),
`docs/gate-oracle-two-key-smoke-test-runbook.md` (runbook), and
`docs/AGENT_FUNDING_PLAYBOOK.md` (reusable lifecycle for future
agents).

## TL;DR

1. Rebuild `novai-cli` on the box. The deployed binary is three weeks
   stale and silently mis-handles the oracle capability flag.
2. Sweep all monitoring, dashboards, and docs for references to the
   dead entity `6c5d016450e49c24eb6ea213e13485923f326f12d9dd19722e07be21ff69bfc9`
   and queue updates to point at the new entity_id (TBD post-deploy).
3. Archive the v1 keyfile on the box. The v1 key is bound to the dead
   Type-8 entity; auto-migration is intentionally not supported.
4. Pull the new agent code; re-run `bootstrap.py`; verify the four
   pass criteria from the smoke runbook.

## Step 1: rebuild novai-cli on the box

The deployed `novai-cli` is three weeks stale. Symptom under v1: the
`--capabilities oracle` flag silently mis-encodes bit 6 because the
older binary predates the alias map at
`tools/novai-cli/src/commands/ai.rs:25-30`. Under v2 the bootstrap
does not shell out to `novai-cli`, but the operator runbook
(`docs/gate-oracle-two-key-smoke-test-runbook.md`) does, so the
rebuild is required before the smoke test runs.

```
# As root on the production box.
# cargo is NOT on the non-interactive ssh PATH; use the absolute path.
[redacted-server]/.cargo/bin/cargo build --release \
    -p novai-cli \
    --manifest-path [redacted-server]/NOVAI-node/NOVAI-node/Cargo.toml
install -m 0755 [redacted-server]/NOVAI-node/NOVAI-node/target/release/novai-cli \
                /usr/local/bin/novai-cli
novai-cli --version
```

Expected: the version string matches the current repo HEAD. If
`cargo` is not found, the operator must run it under `bash -lc` to
pick up the login PATH.

## Step 2: sweep references to the dead entity

The dead oracle entity_id is
`6c5d016450e49c24eb6ea213e13485923f326f12d9dd19722e07be21ff69bfc9`.
Run the sweep from the operator's workstation before the deploy:

```
# Locations to check, by precedence:
# 1. Grafana dashboards (the oracle-specific panels reference entity_id).
# 2. Prometheus alert rules / Alertmanager routes.
# 3. Internal docs / runbooks under docs/ and the wiki.
# 4. Any external API consumer hard-coding the entity_id.

rg -n '6c5d0164' --type-add 'cfg:*.{yml,yaml,json,toml,md}' --type cfg
```

The new entity_id is not known until `bootstrap.py` runs and the
Type-10 registration commits. Capture the new value from the bootstrap
summary output (the `entity_id:` line), then run a second sweep with
the new value to verify the updates landed.

Do not delete the dead-entity references silently; the audit trail
matters. Update each reference to point at the new entity_id and add
a comment noting the migration date.

## Step 3: archive the v1 keyfile

The v1 keyfile at `/etc/novai/oracle-keys.json` holds the single seed
bound to the dead Type-8 entity. The new bootstrap will refuse to
load it (`KeyFileVersionError` at `bootstrap.py` load time, exit code
2). Archive before the agent code update so a partial deploy cannot
silently regenerate the v1 file at the same path.

```
# As root on the production box.
ts=$(date -u +%Y%m%dT%H%M%SZ)
install -m 0600 /etc/novai/oracle-keys.json \
                /etc/novai/oracle-keys.v1.archived-$ts.json
rm /etc/novai/oracle-keys.json
ls -l /etc/novai/oracle-keys*.json
```

Expected: the archive exists at 0600, the live keyfile is gone, no
other `oracle-keys*` files in `/etc/novai/`.

If the operator skips this step, `bootstrap.py` exits 2 with the
clear `KeyFileVersionError` message and no chain state is touched.

## Step 4: pull agent code and re-run bootstrap

```
# As root on the production box.
cd [redacted-server]/NOVAI-node/NOVAI-node
git fetch origin
git checkout main
git pull --ff-only

# Re-install the agent code into the venv path.
install -m 0755 [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/bootstrap.py \
                [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/oracle.py \
                /opt/novai-price-oracle/
install -m 0644 [redacted-server]/NOVAI-node/NOVAI-node/agents/price-oracle/lib/*.py \
                /opt/novai-price-oracle/lib/

# Run bootstrap with the systemd env loaded.
set -a && source /etc/novai/oracle.env && set +a
/opt/novai-price-oracle/.venv/bin/python /opt/novai-price-oracle/bootstrap.py
```

Expected summary output:

```
price-oracle bootstrap complete
  funder_address:   <fresh 64-hex>
  funder_pubkey:    <64-hex>
  funder_balance:   ~ (faucet drop) - 55000
  entity_address:   <fresh 64-hex, distinct from funder_address>
  entity_pubkey:    <64-hex>
  entity_id:        <new entity_id>
  capabilities:     0x47
  post_oracle_anchors (bit 6): True
  registered_at:    <unix>
```

Capture the new `entity_id` for the Step 2 second-pass sweep.

## Step 5: post-deploy verification

This is the integration test: the four pass criteria from
`docs/gate-oracle-two-key-smoke-test-runbook.md` must all hold against
the freshly bootstrapped entity. Run the verification as the
non-systemd operator from the runbook, substituting the production
entity_id and funder address.

```
RPC=http://localhost:3030
ENTITY_ID=<value from bootstrap summary>
FUNDER_ADDR=<value from bootstrap summary>

# 1. ai info reflects an active entity with the right caps.
novai-cli --endpoint "$RPC" --json ai info --entity-id "$ENTITY_ID" \
    | jq '{is_active, capabilities, economic_balance, pubkey, creator}'

# 2. funder behaves like a plain account (nonce climbed, balance reduced).
novai-cli --endpoint "$RPC" --json balance --address "$FUNDER_ADDR" \
    | jq '{balance, nonce}'

# 3. After the systemd service starts, an OracleAnchor commits within
#    PRICE_ORACLE_LOOP_INTERVAL_SECS seconds. Verify via the metrics
#    endpoint and a chain spot-check.
curl -sS http://localhost:9201/metrics | grep -E '^novai_oracle_(submission_success_total|last_submission_height) '

# 4. Entity nonce climbs (was 0 at registration, will increment on
#    each committed signal).
novai-cli --endpoint "$RPC" --json ai info --entity-id "$ENTITY_ID" \
    | jq '.nonce'
```

Pass criteria: `is_active = true`, `capabilities = 71` (0x47),
`pubkey` matches `entity_pubkey` from bootstrap output, `creator`
matches `funder_address`, `economic_balance` is roughly
`50000 - (anchors_committed * 1000)`, funder `nonce >= 2`,
`novai_oracle_submission_success_total >= 1` within five minutes of
service start, entity `nonce >= 1` after first commit.

If any criterion fails, capture the JSON outputs and the systemd
journal:

```
journalctl -u novai-price-oracle -n 200 --no-pager > /tmp/oracle-deploy-tail.log
```

and stop. Do not start the service in production until all four
criteria pass.

## Step 6: enable the service and confirm

```
systemctl daemon-reload
systemctl enable --now novai-price-oracle
systemctl status novai-price-oracle
journalctl -u novai-price-oracle -f
```

Watch the journal for `submit event=ok` messages on the configured
interval. The first successful submit confirms the deploy.

## Rollback

The migration is one-way at the chain layer: the dead Type-8 entity
sits inert in state forever (no cleanup mechanism), and the new
Type-10 entity registered in Step 4 cannot be un-registered.

Rolling back the AGENT CODE is straightforward:

```
# As root on the production box.
cd [redacted-server]/NOVAI-node/NOVAI-node
git checkout <pre-migration commit>
install -m 0755 .../bootstrap.py .../oracle.py /opt/novai-price-oracle/

# Restore the v1 keyfile.
ts=$(ls -1 /etc/novai/oracle-keys.v1.archived-*.json | tail -n1)
install -m 0600 "$ts" /etc/novai/oracle-keys.json

systemctl restart novai-price-oracle
```

The rolled-back service runs against the dead Type-8 entity again
(same failure mode as before this migration). The Type-10 entity from
Step 4 is abandoned and stays inert in state. There is no way to
"unmigrate" the chain side; rollback only restores the agent code.

If rollback is needed, capture the reason in
`docs/gate-oracle-funding-model-diagnosis.md` follow-up so the next
migration attempt has the failure mode on record.
