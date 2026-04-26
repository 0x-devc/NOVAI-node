# Interact with NOVAI from JavaScript

A working TypeScript example that drives a local NOVAI node end-to-end through `@novai/sdk`: connect, fund an account, transfer tokens, register an AI entity, and verify it on chain. ~150 lines, runs in well under a minute.

If this is your first time on NOVAI, run through [`docs/tutorials/FIRST_AI_ENTITY.md`](../../../../docs/tutorials/FIRST_AI_ENTITY.md) first — it sets up the local devnet this example talks to.

---

## Prerequisites

- **A running local devnet** on `http://localhost:3030`. From the repo root:

  ```bash
  ./scripts/devnet.sh
  ```

  Leave that running in another terminal. (See `FIRST_AI_ENTITY.md` Step 2 if anything looks wrong.)

- **Node.js ≥ 18** — `node --version`. The SDK uses the global `fetch` introduced in Node 18.

- **npm** — for installing example dependencies.

---

## Step 1 — Build the SDK (one time)

The example links to `@novai/sdk` via a relative path (`file:../..`), which means the SDK's `dist/` directory must exist before the example can resolve the import.

```bash
cd sdk/novai-sdk-ts
npm install   # only the first time, or after pulling SDK changes
npm run build
```

`tsc` writes the compiled JavaScript + type declarations under `dist/`. Subsequent rebuilds are fast.

---

## Step 2 — Bootstrap the example

```bash
cd examples/quick-start
npm install
```

This pulls in `@novai/sdk` from the local path, plus `tsx` (the runner) and `typescript` (for the local `tsconfig.json`).

> **If you change the SDK source and rebuild it, run `rm -rf node_modules package-lock.json && npm install` here.** npm's `file:` dependency resolution caches aggressively; a force-reinstall picks up the new build cleanly.

---

## Step 3 — Tour `index.ts`

The example is a single file. Each section below maps to a labelled block in `index.ts`.

### 3.1 Connect

```typescript
const client = new NovaiClient("http://localhost:3030");

const latest = await client.call("novai_getLatestBlock", {}) as LatestBlock | null;
if (!latest) throw new Error("Chain has not committed any blocks yet…");
console.log(`Connected. Chain at height ${latest.height}.`);
```

`NovaiClient` wraps every JSON-RPC method the node exposes. `call()` is the escape hatch for raw RPC calls — useful for endpoints the SDK hasn't yet wrapped, like `getLatestBlock`.

### 3.2 Generate keypairs

```typescript
const sender = generateKeypair();
const recipient = generateKeypair();
```

`generateKeypair()` returns `{ seed, publicKey, address }` — all `Uint8Array`. The address is `blake3("NOVAI_ADDRESS_V1" || publicKey)`. Use `bytesToHex(addr)` whenever you need the lowercase hex form RPC speaks.

### 3.3 Fund the sender

```typescript
const faucetResult = await client.faucet(bytesToHex(sender.address));
await sleep(1500); // wait for inclusion
const { balance, nonce } = await client.getBalance(bytesToHex(sender.address));
```

The faucet only works on a node launched with `--dev-keys --allow-insecure-dev-keys` (which `scripts/devnet.sh` provides). It dispenses 10,000,000 tokens per call, with a 1-hour cooldown per address and a 10-second global cooldown.

### 3.4 Transfer

```typescript
const tx = transfer(sender, nonce, 1_000n /* fee */, recipient.address, 100_000n /* amount */);
const txid = await client.submitTx(tx);
```

Tx builders return a fully signed `TxV1`. Numeric fields are `bigint` (`1_000n`, not `1000`). The order is **(keypair, nonce, fee, …type-specific args)** — the same shape across every builder.

### 3.5 Register an AI entity

```typescript
const entityKey = generateKeypair();
const codeHash = new Uint8Array(32).fill(0x01);  // opaque placeholder

const regTx = registerAiEntityWithKey(
  sender,
  nonce,
  5_000n,                           // fee — must meet MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY
  codeHash,
  entityKey.publicKey,              // the entity's own signing key
  AutonomyMode.Gated,
  { readPublicChain: true, readMemoryObjects: true, emitProposals: true },
  50_000n,                          // initial entity balance
);
await client.submitTx(regTx);
```

Two keys are involved: the **creator's** key (which pays the registration fee out of its account) and the **entity's** key (which the entity will use to sign its own future transactions — signal publishes, memory writes, etc.).

### 3.6 Verify on chain

```typescript
const entityId = computeEntityId(codeHash, sender.address);
const entity = await client.getAiEntity(bytesToHex(entityId));
```

`computeEntityId()` mirrors the chain's deterministic derivation: `blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)`. You can compute this client-side without ever talking to a node — useful for predicting the entity id before you submit the registration tx.

---

## Step 4 — Run it

```bash
npm start
```

Real output from a single run on a fresh devnet:

```
Connected. Chain at height 7080.

Sender    address: ddec27d90dcef48a6c09136ee629779e508a32e0df89ebfa9142f5d32d3fc3bc
Recipient address: 030ee0eb3be7d74e387d9ff8d3317ae88235c9248c3d598d8ff240701469fd82

Faucet dispensed 10000000 tokens (tx 747cad18bc0ebede…).
Sender balance: 10000000, nonce: 0

Transfer 100000 tokens → recipient submitted (tx f8330560ed3a1c18…).
Sender    balance: 9899000 (was 10000000)
Recipient balance: 100000

Entity registration submitted (tx 0251dfd4ba2c87b5…).

Entity 5dcbe69fda29a135… on chain:
  creator:        ddec27d90dcef48a6c09136ee629779e508a32e0df89ebfa9142f5d32d3fc3bc
  pubkey:         7999977ed56688c4ad0fa12193a3bf42bc2915b5ca6a769a2808b213a23809ad
  balance:        50000
  autonomy_mode:  1 (Gated)
  capabilities:   0x07
  registered_at:  block 7125
  is_active:      true
```

Walking the numbers:

| Account | Before | After | Δ |
|---|---|---|---|
| Sender | `10_000_000` | `9_899_000` | `−101_000` (= 100,000 transfer + 1,000 fee) |
| Recipient | `0` | `100_000` | `+100_000` |

The sender's balance after registration would be `9_899_000 − 50_000 − 5_000 = 9_844_000` — the example doesn't print it, but `client.getBalance(...)` would confirm.

`capabilities: 0x07` is bits 0+1+2 set — `read_public_chain`, `read_memory_objects`, `emit_proposals` — the three flags we passed.

---

## Troubleshooting

- **`Error: Cannot find module '@novai/sdk'`** — the SDK's `dist/` is missing or stale. Run `npm run build` in `sdk/novai-sdk-ts/`, then `rm -rf node_modules package-lock.json && npm install` here.
- **`Chain has not committed any blocks yet`** — the devnet hasn't reached its first commit. With four validators it usually takes 1–2 seconds; if it lingers, check `/tmp/node{0,1,2,3}.log`.
- **`RPC error -32601: Method not found`** — the node is older than the SDK and doesn't speak the method. Update both to the same revision.
- **Faucet fails with "dev mode disabled"** — the node was started without `--dev-keys --allow-insecure-dev-keys`. Use `scripts/devnet.sh` (which sets these), not a hand-rolled launch.

---

## What's next

- **Rust SDK tutorial** — same flow in Rust against `novai-sdk` (Phase 1.3, in progress).
- **Build a bot** — extend this example to publish signals and own memory objects (Phase 3.2 of the roadmap).
- **Full RPC reference** — every method the node exposes, with request/response shapes (Phase 1.4, in progress).
