# NOVAI 2-minute walkthrough — recording script

A cold-open to "AI entities are protocol primitives": on-chain identity, on-chain memory, on-chain signals — no smart contracts in the middle.

Target length **120 seconds** (~300 words spoken at ~150 wpm). Cuts every 10–15 seconds to keep momentum.

---

## Scene plan

### 0:00 – 0:10 · Cold open

**On screen.** Black. White text fades in: *"What if AI agents were protocol primitives, not smart contracts?"* — hold 3s — fade.

**Voiceover.** "Most blockchains treat AI agents as smart contracts deployed on top. NOVAI treats them as first-class on-chain identities."

### 0:10 – 0:25 · The chain

**On screen.** Terminal — `./scripts/devnet.sh` running, four validators committing blocks. Briefly cut to `tail -f /tmp/node0.log` showing `COMMITTED block height=…` lines flying by.

**Voiceover.** "This is a four-validator NOVAI devnet. Rust, BFT consensus, deterministic execution. About seven blocks per second locally. Standard L1 plumbing."

### 0:25 – 0:55 · Register an AI entity

**On screen.** Run `bash demos/ai-entity-demo.sh` — narrate the banner sections as they fly by. Slow the camera at the `ai info` block to highlight: id, pubkey, autonomy_mode, capabilities, balance.

**Voiceover.** "Register an entity. Two keys: a creator key that pays the fee, and an entity key the agent will sign with. The chain mints a deterministic id, stores the entity's pubkey, and gives it an economic balance — ten thousand tokens, paid out of the creator. The entity now exists at the protocol level."

### 0:55 – 1:25 · The entity acts

**On screen.** Cut to the same terminal as the script publishes a signal and creates a memory object. Then jump to the explorer (`http://localhost:5173/entity/<id>`) — the signal appears in the table; the memory object appears below.

**Voiceover.** "The entity publishes a signal — a content-addressed hash, signed by the entity, indexed by the chain. It writes a memory object — durable on-chain content, capped at sixty-four kilobytes, owned by this entity. No smart contract was deployed. No off-chain database was touched. The chain itself is the integration surface."

### 1:25 – 1:50 · Multi-entity

**On screen.** Split screen — left terminal running `npm run predictor`, right terminal running `npm run risk-scorer`. Show a few prediction → risk-score pairs streaming. Cut to the explorer showing both entities side by side, each with their own signals.

**Voiceover.** "Two entities now. The predictor publishes prediction signals. The risk-scorer reads them, looks up the actual block, scores the prediction, and publishes its own signal in response. Anyone can join — call `getSignalsByIssuer`, act on what you see, write your own. No API. No event bus. Just chain."

### 1:50 – 2:00 · Wrap

**On screen.** Cut to the explorer Stats page — show height ticking, blocks-per-second, recent transaction count. Fade to logo + tagline.

**Voiceover.** "AI-native L1. AI entities as protocol primitives. Hello."

---

## Recording checklist

- [ ] Terminal: a clean **dark theme**, font ≥ 16pt, narrow window so commands wrap nicely
- [ ] Browser: explorer in **dark mode** (already default), zoomed to ~125% so addresses are readable in the recording
- [ ] Devnet started fresh — `rm -rf ~/.novai/data && ./scripts/devnet.sh` — so block heights are small
- [ ] Mute notifications, kill anything that might pop a banner
- [ ] Record at 1080p minimum, 30 fps
- [ ] Record system audio off; voice-over recorded separately, layered in post

---

## Recording sequence (one take, in order)

1. Start devnet in tab 1. Wait for first commit.
2. Open explorer in browser tab. Confirm latest block is showing.
3. In tab 2, run `bash demos/ai-entity-demo.sh`. Pause at the `ai info` block long enough for the camera to read it.
4. Switch to the explorer browser tab, paste the entity id, show its page populating.
5. Back to terminal — start predictor in tab 3.
6. Wait ~20 s for predictor to publish at least 2 signals.
7. Start risk-scorer in tab 4.
8. Wait ~30 s for the first risk score to land.
9. Switch to explorer — show both entity pages with their signals.
10. End on the Stats page.

Total clean run-time: about 90 seconds; the rest is voiceover pacing.

---

## Posting kit

### Twitter thread (4 tweets)

> 1/ Most chains treat AI agents as smart contracts. NOVAI treats them as protocol primitives.
>
> Watch a fresh devnet register an AI entity, publish a signal, store a memory object — no contracts deployed. <link>

> 2/ Each entity has its own ed25519 key, its own balance, its own nonce. It signs its own transactions. The chain indexes its signals + memory objects natively.

> 3/ Multi-entity composition is just chain reads. One bot publishes predictions, another scores them by reading `getSignalsByIssuer`. No off-chain API surface to coordinate against.

> 4/ Rust, HotStuff BFT, deterministic execution. Four-validator local devnet boots in five seconds. Code + tutorial: <repo URL>

### dev.to post outline

- **Title.** "AI entities as protocol primitives: a walkthrough"
- **Hook.** Open with the contrast — smart contracts vs on-chain identity.
- **Section 1.** Show the CLI registering an entity end-to-end (paste the demo script transcript).
- **Section 2.** Show the explorer rendering it.
- **Section 3.** Show the multi-entity demo — emphasise that the integration is *the chain*.
- **Section 4.** Architecture pointer — link to `docs/ARCHITECTURE.md` for readers who want the depth.
- **CTA.** Repo link, Quick Start tutorial link, "open issues welcome".

---

## Notes for revision

- If the demo runs faster than expected, fill the time with a **second pass** through the explorer pages: blocks → block detail → tx detail → account.
- If the demo runs slower (faucet cooldown hits etc.), cut the credit-entity step from `ai-entity-demo.sh` for the recording and re-narrate.
- Voiceover script is intentionally tight (~300 words). If a recording comes in long, tighten 0:55–1:25 by dropping the "no smart contract was deployed" repetition.
