# NOVAI anomaly-detection bot

A small TypeScript demo of the "AI-entity-as-protocol-primitive" pattern: the bot is itself an on-chain entity that watches chain activity, evaluates three simple heuristics, and publishes anomaly signals + memory objects whenever one fires.

It's a demo — the heuristics are deliberately simple and the bot is single-validator-aware (it doesn't try to coordinate with other watchers). The point is the shape of the integration, not the depth of the analytics.

---

## What it does

Each poll (~1.5 s):

1. Calls `novai_getLatestBlock`, appends the result to a 50-block sliding window.
2. Runs three detectors over the window:
   - **`empty-streak`** — last 30 blocks all had `tx_count == 0` (chain is idle).
   - **`stalled`** — head height hasn't changed in > 15 s (consensus halt).
   - **`leader-rotation`** — any of the last 10 blocks committed at round > 0 (leader timeout).
3. If a detector fires *and* its per-kind cooldown has elapsed, the bot:
   - Publishes a `SignalType.Anomaly` signal with a domain-tagged hash of the detection details.
   - Writes an `AnomalyLog` memory object containing the JSON details.

Cooldowns (in blocks):

| Kind | Cooldown |
|---|---|
| `empty-streak` | 60 |
| `stalled` | 30 |
| `leader-rotation` | 60 |

---

## Run it

```bash
# 1. Build the SDK once (skip if already built).
cd sdk/novai-sdk-ts && npm install && npm run build && cd -

# 2. Make sure a devnet is running.
./scripts/devnet.sh   # in another terminal

# 3. Start the bot.
cd demos/anomaly-bot
npm install
npm start
```

First run: the bot generates a creator + entity keypair, faucets the creator, registers the entity, and writes `bot-state.json` (gitignored). Subsequent runs reload from that file and skip registration.

To watch from the explorer side: open `http://localhost:5173/entity/<entity_id>` (the bot prints its entity id at startup) — you'll see signals stream in and the memory-object count tick up.

### Knobs

| Env var | Default | Effect |
|---|---|---|
| `NOVAI_RPC_URL` | `http://localhost:3030` | Where the bot talks |
| `BOT_STATE_PATH` | `./bot-state.json` | Where keys live (sensitive — chmod 0600) |
| `BOT_RUN_FOR_MS` | `0` (forever) | Auto-exit after N ms; useful for CI/demos |

### Stopping

Ctrl+C. The bot doesn't write any cleanup state; the next start re-uses the same entity.

To rotate identity, delete `bot-state.json` and start over.

---

## Limitations

- **No leader-rotation in normal devnet.** With four healthy local validators, blocks always commit at round 0. To force the `leader-rotation` detector to fire, kill one or two of the four validators and watch the remaining ones rotate — round will start incrementing.
- **`empty-streak` fires almost immediately on a fresh devnet.** The chain commits empty blocks at ~7 blocks/sec by default; without traffic, 30 empties take ~5 s. Run `tools/tx-generator` alongside if you want the bot to *not* fire.
- **`stalled` requires you to actually halt the chain.** Easiest: `pkill -f 'novai-node run'`. The bot will publish a `stalled` signal once the latest-block timestamp ages past 15 s (it will fail to publish, since the chain is down — that's fine; the next start will recover).
- **Cooldowns are in-memory.** Restarting the bot resets them, so a freshly restarted bot may immediately re-publish the same kind of anomaly.

---

## Design notes

- **Why publish to chain at all?** Because that's the demo: signals + memory objects are first-class on-chain artifacts. Any other observer (another bot, a wallet, the explorer) sees the bot's outputs without needing to talk to its API.
- **Why bound memory writes via cooldowns?** Each entity is capped at 100 memory objects on chain. A bot publishing once per anomaly per cooldown window keeps that bounded.
- **Why a separate creator and entity key?** The creator is the funding account (a normal user account). The entity has its own ed25519 key so the chain can verify entity-signed signal/memory transactions independently of who registered it.
