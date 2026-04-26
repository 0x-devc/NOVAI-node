/**
 * NOVAI anomaly-detection bot.
 *
 * Registers itself as an on-chain AI entity, polls the chain every poll
 * interval, evaluates three simple heuristics, and publishes a signal +
 * memory object whenever one fires. Cooldowns prevent re-firing on the
 * same condition.
 *
 * Run: `npm install && npm start` (after building the SDK once with
 * `cd ../../sdk/novai-sdk-ts && npm install && npm run build`).
 */

import { createHash } from "blake3";
import {
  AutonomyMode,
  bytesToHex,
  computeEntityId,
  createMemory,
  generateKeypair,
  hexToBytes,
  keypairFromSeed,
  MemoryObjectType,
  NovaiClient,
  registerAiEntityWithKey,
  signalCommitment,
  SignalType,
} from "@novai/sdk";
import {
  detect,
  EMPTY_STREAK_THRESHOLD,
  type Anomaly,
  type AnomalyKind,
} from "./detector";
import { loadState, saveState, type BotState } from "./store";

const RPC_URL = process.env.NOVAI_RPC_URL ?? "http://localhost:3030";
const STATE_PATH = process.env.BOT_STATE_PATH ?? "./bot-state.json";
const RUN_FOR_MS = parseInt(process.env.BOT_RUN_FOR_MS ?? "0", 10); // 0 = forever
const POLL_MS = 1500;
const WINDOW_SIZE = Math.max(EMPTY_STREAK_THRESHOLD, 50);
const CODE_HASH = new Uint8Array(32).fill(0xa1);
const ENTITY_INITIAL_BALANCE = 500_000n;
const REGISTER_FEE = 5_000n;
const SIGNAL_FEE = 1_000n;
const MEMORY_FEE = 500n;
const COOLDOWN_BLOCKS: Record<AnomalyKind, number> = {
  "empty-streak": 60,
  "stalled": 30,
  "leader-rotation": 60,
};

interface LatestBlock {
  height: number;
  round: number;
  tx_count: number;
  block_hash: string;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function commitmentHash(input: string): Uint8Array {
  const h = createHash();
  h.update(Buffer.from(input, "utf-8"));
  return new Uint8Array(h.digest());
}

async function bootstrap(client: NovaiClient): Promise<{
  state: BotState;
  isFresh: boolean;
}> {
  const existing = loadState(STATE_PATH);
  if (existing) {
    log("info", `loaded existing bot state from ${STATE_PATH}`);
    return { state: existing, isFresh: false };
  }

  log("info", "first run — generating keypairs and registering on chain");

  const creator = generateKeypair();
  const entity = generateKeypair();

  // Faucet the creator.
  log("info", "requesting faucet for creator");
  await client.faucet(bytesToHex(creator.address));
  await sleep(1500);

  // Register the entity.
  const nonce = await client.getNonce(bytesToHex(creator.address));
  const tx = registerAiEntityWithKey(
    creator,
    nonce,
    REGISTER_FEE,
    CODE_HASH,
    entity.publicKey,
    AutonomyMode.Gated,
    {
      readPublicChain: true,
      readMemoryObjects: true,
      emitProposals: true,
    },
    ENTITY_INITIAL_BALANCE,
  );
  const txid = await client.submitTx(tx);
  log("info", `register tx submitted: ${txid.slice(0, 16)}…`);
  await sleep(2000);

  const entityId = computeEntityId(CODE_HASH, creator.address);
  const state: BotState = {
    creatorSeedHex: bytesToHex(creator.seed),
    entitySeedHex: bytesToHex(entity.seed),
    entityIdHex: bytesToHex(entityId),
  };
  saveState(STATE_PATH, state);
  log("info", `bot entity registered: ${state.entityIdHex.slice(0, 16)}…`);
  return { state, isFresh: true };
}

interface BlockSnapshot {
  height: number;
  round: number;
  tx_count: number;
  observedAt: number;
}

async function pollLatest(client: NovaiClient): Promise<LatestBlock | null> {
  return (await client.call("novai_getLatestBlock", {})) as LatestBlock | null;
}

function log(level: "info" | "anomaly" | "warn", msg: string): void {
  const ts = new Date().toISOString().slice(11, 19);
  const tag =
    level === "anomaly" ? "🚨 ANOMALY" : level === "warn" ? "⚠ WARN" : "  info";
  console.log(`[${ts}] ${tag}  ${msg}`);
}

async function publishAnomaly(
  client: NovaiClient,
  state: BotState,
  anomaly: Anomaly,
): Promise<void> {
  const entity = keypairFromSeed(hexToBytes(state.entitySeedHex));
  const detailJson = JSON.stringify(anomaly.detail);
  const signalInput = `${anomaly.kind}|${anomaly.height}|${detailJson}`;
  const sigHash = commitmentHash(signalInput);

  // Signal first (cheaper, faster to query). Use entity nonce.
  const entityIdBytes = hexToBytes(state.entityIdHex);
  const entityRecord = await client.getAiEntity(state.entityIdHex);
  if (!entityRecord) throw new Error("entity record vanished");
  let nonce = BigInt(entityRecord.nonce);

  const sigTx = signalCommitment(
    entity,
    nonce,
    SIGNAL_FEE,
    sigHash,
    SignalType.Anomaly,
    entityIdBytes,
  );
  const sigTxid = await client.submitTx(sigTx);
  log(
    "anomaly",
    `${anomaly.kind} @ height ${anomaly.height} → signal ${sigTxid.slice(0, 12)}…`,
  );
  await sleep(1500);

  // Memory object capturing the detail.
  nonce += 1n;
  const memTx = createMemory(
    entity,
    nonce,
    MEMORY_FEE,
    MemoryObjectType.AnomalyLog,
    Buffer.from(JSON.stringify({ kind: anomaly.kind, ...anomaly.detail, height: anomaly.height })),
  );
  const memTxid = await client.submitTx(memTx);
  log(
    "info",
    `  memory object stored (${detailJson.length} bytes detail) → ${memTxid.slice(0, 12)}…`,
  );
}

async function main(): Promise<void> {
  console.log("NOVAI anomaly bot starting");
  console.log(`  rpc:  ${RPC_URL}`);
  console.log(`  poll: ${POLL_MS}ms`);
  console.log(`  state: ${STATE_PATH}`);
  if (RUN_FOR_MS > 0) console.log(`  will exit after ${RUN_FOR_MS}ms`);
  console.log();

  const client = new NovaiClient(RPC_URL);
  const { state } = await bootstrap(client);

  const window: BlockSnapshot[] = [];
  let lastChangedAt = Date.now();
  let lastSeenHeight = -1;
  const lastFiredAtHeight: Partial<Record<AnomalyKind, number>> = {};
  const startedAt = Date.now();

  log("info", `entity ${state.entityIdHex.slice(0, 16)}… polling…`);

  // eslint-disable-next-line no-constant-condition
  while (true) {
    if (RUN_FOR_MS > 0 && Date.now() - startedAt > RUN_FOR_MS) {
      log("info", "run duration elapsed, exiting cleanly");
      return;
    }

    try {
      const head = await pollLatest(client);
      if (!head) {
        log("warn", "chain has no committed blocks yet");
        await sleep(POLL_MS);
        continue;
      }

      if (head.height !== lastSeenHeight) {
        // Fetch every block we missed since the last poll so the window
        // reflects "consecutive blocks", not "sampled blocks".
        const fetchFrom =
          lastSeenHeight === -1 ? head.height : lastSeenHeight + 1;
        const missed: number[] = [];
        for (let h = fetchFrom; h <= head.height; h++) missed.push(h);
        // Cap how many we backfill in one go to avoid hammering the node
        // after long pauses; the head sample is always included.
        const toFetch = missed.slice(-WINDOW_SIZE);
        const observedAt = Date.now();
        const headers = await Promise.all(
          toFetch.map((h) =>
            (
              client.call("novai_getBlockByHeight", { height: h }) as Promise<{
                height: number;
                round: number;
                tx_count: number;
              } | null>
            ).catch(() => null),
          ),
        );
        // Push newest-first.
        for (const b of headers.reverse()) {
          if (!b) continue;
          window.unshift({
            height: b.height,
            round: b.round,
            tx_count: b.tx_count,
            observedAt,
          });
        }
        if (window.length > WINDOW_SIZE) window.length = WINDOW_SIZE;
        lastChangedAt = observedAt;
        lastSeenHeight = head.height;
      }

      const anomaly = detect(window, lastChangedAt);
      if (anomaly) {
        const since = lastFiredAtHeight[anomaly.kind] ?? -Infinity;
        const cooldown = COOLDOWN_BLOCKS[anomaly.kind];
        if (anomaly.height - since >= cooldown) {
          await publishAnomaly(client, state, anomaly);
          lastFiredAtHeight[anomaly.kind] = anomaly.height;
        }
      }
    } catch (err) {
      log("warn", `poll error: ${err instanceof Error ? err.message : err}`);
    }

    await sleep(POLL_MS);
  }
}

main().catch((err: unknown) => {
  console.error("fatal:", err instanceof Error ? err.message : err);
  process.exit(1);
});
