/**
 * Risk-scorer (Bot B).
 *
 * Watches the predictor's signals via getSignalsByIssuer, fetches each
 * prediction's detail from the predictor's memory objects, and once the
 * target height is reached compares predicted vs. actual block tx_count.
 * Publishes a SignalType.RiskScore signal carrying the delta.
 *
 * Reads the predictor's entity_id from predictor-state.json (written by
 * the predictor on its first run). If that file doesn't exist yet, the
 * risk-scorer waits for it.
 */

import {
  hexToBytes,
  MemoryObjectInfo,
  NovaiClient,
  signalCommitment,
  SignalType,
} from "@novai/sdk";
import {
  bootstrap,
  commitmentHash,
  entityNonce,
  loadState,
  RPC_URL,
  ts,
} from "./shared";

const STATE_PATH = "./risk-scorer-state.json";
const PREDICTOR_STATE_PATH = "./predictor-state.json";
const POLL_INTERVAL_MS = 5_000;
const SIGNAL_FEE = 1_000n;
const SIGNAL_LOOKBACK_BLOCKS = 5_000;
const RUN_FOR_MS = parseInt(process.env.RUN_FOR_MS ?? "0", 10);

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface LatestBlock {
  height: number;
}

interface BlockHeader {
  height: number;
  tx_count: number;
}

async function waitForPredictor(): Promise<string> {
  // Poll the predictor state file until it appears.
  while (true) {
    const s = loadState(PREDICTOR_STATE_PATH);
    if (s) return s.entityIdHex;
    console.log(
      `[${ts()}] [risk-scorer] waiting for predictor (no ${PREDICTOR_STATE_PATH} yet)…`,
    );
    await sleep(3000);
  }
}

async function getActualTxCount(
  client: NovaiClient,
  height: number,
): Promise<number | null> {
  const block = (await client.call("novai_getBlockByHeight", { height })) as
    | BlockHeader
    | null;
  return block?.tx_count ?? null;
}

async function main(): Promise<void> {
  console.log("NOVAI multi-entity demo: RISK-SCORER (Bot B)");
  console.log(`  rpc:    ${RPC_URL}`);
  console.log(`  state:  ${STATE_PATH}`);
  console.log(`  poll:   every ${POLL_INTERVAL_MS / 1000}s`);
  console.log();

  const client = new NovaiClient(RPC_URL);
  const predictorEntityIdHex = await waitForPredictor();
  console.log(
    `[${ts()}] [risk-scorer] predictor is ${predictorEntityIdHex.slice(0, 16)}…`,
  );

  const { state, entity } = await bootstrap(
    client,
    STATE_PATH,
    0xb2,
    "risk-scorer",
  );

  // Pairing predictor signals to predictor memory objects by emission order
  // is unreliable: getMemoryObjects returns memos sorted by object_id (a
  // content hash), not by the order they were created. We instead drive
  // scoring directly off the memo content — each memo carries its own
  // `target_height`, which uniquely identifies the prediction.
  const scoredTargets = new Set<number>();
  const startedAt = Date.now();

  while (true) {
    if (RUN_FOR_MS > 0 && Date.now() - startedAt > RUN_FOR_MS) {
      console.log(`[${ts()}] [risk-scorer] run duration elapsed, exiting`);
      return;
    }

    try {
      const head = (await client.call("novai_getLatestBlock", {})) as
        | LatestBlock
        | null;
      if (!head) {
        await sleep(POLL_INTERVAL_MS);
        continue;
      }

      const memos = await client.getMemoryObjects(predictorEntityIdHex);

      interface Prediction {
        target_height: number;
        predicted_tx_count: number;
        memo: MemoryObjectInfo;
      }

      const predictions: Prediction[] = [];
      for (const m of memos) {
        try {
          const detail = JSON.parse(
            Buffer.from(m.data, "hex").toString("utf-8"),
          ) as { target_height?: number; predicted_tx_count?: number };
          if (
            typeof detail.target_height === "number" &&
            typeof detail.predicted_tx_count === "number"
          ) {
            predictions.push({
              target_height: detail.target_height,
              predicted_tx_count: detail.predicted_tx_count,
              memo: m,
            });
          }
        } catch {
          /* not a prediction memo — skip */
        }
      }

      // Score oldest first so we publish in chronological order.
      predictions.sort((a, b) => a.target_height - b.target_height);

      for (const p of predictions) {
        if (scoredTargets.has(p.target_height)) continue;
        if (head.height < p.target_height) continue;

        const actual = await getActualTxCount(client, p.target_height);
        if (actual === null) continue;

        const delta = Math.abs(actual - p.predicted_tx_count);
        const score = Math.min(100, delta * 25); // arbitrary toy formula

        const scoreInput = `risk|${p.memo.object_id}|${p.target_height}|${actual}|${score}`;
        const scoreHash = commitmentHash(scoreInput);

        const nonce = await entityNonce(client, state.entityIdHex);
        const tx = signalCommitment(
          entity,
          nonce,
          SIGNAL_FEE,
          scoreHash,
          SignalType.RiskScore,
          hexToBytes(state.entityIdHex),
        );
        const txid = await client.submitTx(tx);

        console.log(
          `[${ts()}] [risk-scorer] block ${p.target_height}: predicted ${p.predicted_tx_count}, actual ${actual} → risk ${score} (sig ${txid.slice(0, 12)}…)`,
        );
        scoredTargets.add(p.target_height);
        await sleep(1500);
      }
    } catch (err) {
      console.log(
        `[${ts()}] [risk-scorer] error: ${err instanceof Error ? err.message : err}`,
      );
    }

    await sleep(POLL_INTERVAL_MS);
  }
}

main().catch((err: unknown) => {
  console.error("fatal:", err instanceof Error ? err.message : err);
  process.exit(1);
});
