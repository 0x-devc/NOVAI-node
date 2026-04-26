# Multi-entity interaction demo

Two AI entities interacting on chain — no out-of-band API. Bot A (`predictor`) publishes prediction signals + memory objects; Bot B (`risk-scorer`) reads them via `getSignalsByIssuer` + `getMemoryObjects` and publishes risk-score signals in response.

The point: both bots are first-class on-chain identities. The signals and memory objects they exchange are durable, indexable, and visible to anyone — the explorer renders them in the same UI used for any other entity.

---

## What each bot does

### Predictor (Bot A) — `npm run predictor`

Every 10 seconds:

1. Reads the latest block height `H`.
2. Picks a target height `H+5` and a toy "prediction" (a deterministic small integer).
3. Publishes:
   - `SignalType.Prediction` with `signal_hash = blake3("prediction|<details>")`.
   - `MemoryObjectType.LabelIndex` carrying `{ target_height, predicted_tx_count, … }`.

Signals indexed under entity `A`, queryable via `novai_getSignalsByIssuer(<A>)`.

### Risk-scorer (Bot B) — `npm run risk-scorer`

Every 5 seconds:

1. Reads `predictor-state.json` (written by Bot A on first run) to learn `A`'s entity id.
2. Calls `novai_getSignalsByIssuer(<A>, …)` for the recent window.
3. For each unscored signal whose target height has been committed, fetches the matching memory object, compares predicted vs actual tx-count, and publishes a `SignalType.RiskScore` signal.

The pairing of signal → memory object is by emission order in this demo — robust pairings would commit the memory `object_id` into the signal hash; left simple here for clarity.

---

## Run it

```bash
# 1. Build the SDK (once).
cd sdk/novai-sdk-ts && npm install && npm run build && cd -

# 2. Start a devnet (separate terminal).
./scripts/devnet.sh

# 3. Install demo deps.
cd demos/multi-entity
npm install

# 4. Start the predictor — it writes predictor-state.json on first run.
npm run predictor    # leave running

# 5. In another terminal, start the risk-scorer.
cd demos/multi-entity
npm run risk-scorer
```

Within 30–60 seconds you should see paired signals streaming. Watch them in the explorer:

- Bot A: `http://localhost:5173/entity/<predictor entity id>`
- Bot B: `http://localhost:5173/entity/<risk-scorer entity id>`

Each bot prints its own entity id at startup; they're also in `predictor-state.json` and `risk-scorer-state.json` (gitignored).

### Stopping

Ctrl+C either bot. Restarting reuses the same entity (state files persist).

To rotate identities, delete the `*-state.json` files and start over.

### Knobs

| Env var | Effect |
|---|---|
| `NOVAI_RPC_URL` | RPC target (default `http://localhost:3030`) |
| `RUN_FOR_MS` | Auto-exit after N ms (useful for CI/demos; 0 = forever) |

---

## What this demonstrates

- **Composability without APIs.** Bot B never makes an HTTP call to Bot A. Their interaction is mediated entirely by chain-recorded signals and memory objects, indexed by the chain.
- **Anyone can join the conversation.** A third bot could publish its own scores against Bot A's predictions tomorrow, with no coordination — just call `getSignalsByIssuer(<A>)` and act.
- **Audit-ability.** Every signal and memory object is permanently recorded with a block height, an issuer, and a content-addressed hash. There's no separate event bus, message queue, or audit log.
- **Per-entity quotas are enforced.** Each entity is capped at 100 memory objects and pays its own fees from its own balance. The economics are isolated per identity.

---

## Limitations

- **The "prediction" is a toy formula** (`(iteration * 7) % 4`). Replace with a real model for a real demo.
- **The pairing of signal ↔ memory object** is by emission order in this demo. A production version would commit `object_id` (or a hash of the memo bytes) into the signal hash so pairing is content-addressed and out-of-order safe.
- **Memory objects are bounded at 100 per entity.** Long-running bots need to call `delete_memory_object` to make room.
