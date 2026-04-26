/**
 * Predictor (Bot A).
 *
 * Every PUBLISH_INTERVAL_MS, picks a near-future block height and a guess
 * for its tx_count, publishes:
 *   - SignalType.Prediction with signal_hash = blake3("prediction|...|nonce")
 *   - MemoryObjectType.LabelIndex carrying { target_height, predicted_tx_count }
 *
 * The risk-scorer reads these signals + memory objects and scores them.
 */

import {
  bytesToHex,
  createMemory,
  hexToBytes,
  MemoryObjectType,
  NovaiClient,
  signalCommitment,
  SignalType,
} from "@novai/sdk";
import {
  bootstrap,
  commitmentHash,
  entityNonce,
  RPC_URL,
  ts,
} from "./shared";

const STATE_PATH = "./predictor-state.json";
const PUBLISH_INTERVAL_MS = 10_000;
const SIGNAL_FEE = 1_000n;
const MEMORY_FEE = 500n;
const RUN_FOR_MS = parseInt(process.env.RUN_FOR_MS ?? "0", 10);

interface LatestBlock {
  height: number;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function publishPrediction(
  client: NovaiClient,
  state: { entitySeedHex: string; entityIdHex: string },
  entity: { seed: Uint8Array; publicKey: Uint8Array; address: Uint8Array },
  iteration: number,
): Promise<void> {
  const head = (await client.call("novai_getLatestBlock", {})) as
    | LatestBlock
    | null;
  if (!head) {
    console.log(`[${ts()}] [predictor] chain has no blocks yet, skipping`);
    return;
  }
  const targetHeight = head.height + 5;
  const predictedTxCount = (iteration * 7) % 4; // deterministic toy "prediction"

  const detail = JSON.stringify({
    target_height: targetHeight,
    predicted_tx_count: predictedTxCount,
    published_at_height: head.height,
    iteration,
  });
  const sigHash = commitmentHash(`prediction|${detail}`);

  let nonce = await entityNonce(client, state.entityIdHex);

  const sigTx = signalCommitment(
    entity,
    nonce,
    SIGNAL_FEE,
    sigHash,
    SignalType.Prediction,
    hexToBytes(state.entityIdHex),
  );
  const sigTxid = await client.submitTx(sigTx);
  console.log(
    `[${ts()}] [predictor] #${iteration} → predicting block ${targetHeight} = ${predictedTxCount} txs (sig ${sigTxid.slice(0, 12)}…)`,
  );
  await sleep(1500);

  nonce += 1n;
  const memTx = createMemory(
    entity,
    nonce,
    MEMORY_FEE,
    MemoryObjectType.LabelIndex,
    Buffer.from(detail),
  );
  await client.submitTx(memTx);
}

async function main(): Promise<void> {
  console.log("NOVAI multi-entity demo: PREDICTOR (Bot A)");
  console.log(`  rpc:   ${RPC_URL}`);
  console.log(`  state: ${STATE_PATH}`);
  console.log(`  cycle: every ${PUBLISH_INTERVAL_MS / 1000}s`);
  console.log();

  const client = new NovaiClient(RPC_URL);
  const { state, entity } = await bootstrap(client, STATE_PATH, 0xa1, "predictor");
  void bytesToHex; // keep import silent if unused

  let i = 0;
  const startedAt = Date.now();
  while (true) {
    if (RUN_FOR_MS > 0 && Date.now() - startedAt > RUN_FOR_MS) {
      console.log(`[${ts()}] [predictor] run duration elapsed, exiting`);
      return;
    }
    try {
      await publishPrediction(client, state, entity, i);
      i++;
    } catch (err) {
      console.log(
        `[${ts()}] [predictor] error: ${err instanceof Error ? err.message : err}`,
      );
    }
    await sleep(PUBLISH_INTERVAL_MS);
  }
}

main().catch((err: unknown) => {
  console.error("fatal:", err instanceof Error ? err.message : err);
  process.exit(1);
});
