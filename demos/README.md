# NOVAI demos

Four self-contained showcases of the AI-entity-as-protocol-primitive pattern.

| Path | What it is | Run |
|---|---|---|
| [`ai-entity-demo.sh`](ai-entity-demo.sh) | End-to-end CLI script: keygen → faucet → register → credit → signal → memory CRUD → query, with banner sections suitable for a blog or video transcript. | `bash demos/ai-entity-demo.sh` |
| [`anomaly-bot/`](anomaly-bot/) | TypeScript bot that registers as an on-chain entity, watches chain activity, and publishes anomaly signals + memory objects when its detectors fire. | `cd demos/anomaly-bot && npm install && npm start` |
| [`multi-entity/`](multi-entity/) | Two TypeScript bots: a predictor publishes prediction signals, a risk-scorer reads them and responds with risk-score signals. Pure on-chain composition. | `cd demos/multi-entity && npm install && npm run predictor` (in one terminal); `npm run risk-scorer` in another |
| [`VIDEO_SCRIPT.md`](VIDEO_SCRIPT.md) | A 2-minute walkthrough script with timing, voiceover, recording checklist, and a Twitter/dev.to posting kit. | n/a — written script |

---

## Prerequisites

All three runnable demos talk to a local NOVAI node:

```bash
./scripts/devnet.sh   # in a separate terminal
```

The TypeScript demos depend on the local SDK at `sdk/novai-sdk-ts`. Build it once before running them:

```bash
cd sdk/novai-sdk-ts && npm install && npm run build
```

The shell demo expects `target/release/novai-cli`:

```bash
cargo build --release -p novai-cli
```

For SDK-level walkthroughs, see [`sdk/novai-sdk/examples/quick-start/`](../sdk/novai-sdk/examples/quick-start/) (Rust) and [`sdk/novai-sdk-ts/examples/quick-start/`](../sdk/novai-sdk-ts/examples/quick-start/) (TypeScript). For the chain itself, start with [`docs/tutorials/FIRST_AI_ENTITY.md`](../docs/tutorials/FIRST_AI_ENTITY.md).

---

## State files (gitignored)

Each TypeScript bot persists its keys + entity id to a JSON file so restarts re-use the same on-chain identity:

| Bot | File |
|---|---|
| anomaly-bot | `demos/anomaly-bot/bot-state.json` |
| predictor | `demos/multi-entity/predictor-state.json` |
| risk-scorer | `demos/multi-entity/risk-scorer-state.json` |

Delete the file to rotate identity. Don't commit any of them.

---

## Watching the demos in the explorer

Once a bot has registered, look it up at `http://localhost:5173/entity/<id>` (start the explorer with `cd explorer && npm run dev`). The entity page renders signals and memory objects in real time.
