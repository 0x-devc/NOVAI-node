#!/usr/bin/env node
// Chain snapshot capture: one novai_getLatestBlock call from Node (no browser
// CORS), written to src/data/chain-snapshot.json. Run manually via
// `npm run snapshot` when fresher numbers are wanted, then commit the file.
// Deliberately NOT wired into prebuild: builds stay deterministic and
// network-free (operator decision, answer 13).

import { writeFileSync, renameSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "data", "chain-snapshot.json");
const URL = process.env.SNAPSHOT_RPC_URL || "https://rpc.novai.network";

function fail(msg) {
  console.error(`chain-snapshot: FAIL: ${msg}`);
  process.exit(1);
}

const controller = new AbortController();
const timer = setTimeout(() => controller.abort(), 15000);

let res;
try {
  res = await fetch(URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", method: "novai_getLatestBlock", params: {}, id: 1 }),
    signal: controller.signal,
  });
} catch (err) {
  fail(`RPC unreachable: ${err.message}`);
} finally {
  clearTimeout(timer);
}

if (!res.ok) fail(`HTTP ${res.status}`);
let body;
try {
  body = await res.json();
} catch {
  fail("non-JSON response");
}
if (body.error) fail(`JSON-RPC error: ${JSON.stringify(body.error)}`);
const r = body.result;
if (!r || typeof r.height !== "number") fail("result missing or malformed");

const snapshot = {
  capturedAt: new Date().toISOString(),
  height: r.height,
  round: r.round,
  txCount: r.tx_count,
  blockHash: r.block_hash,
  parentHash: r.parent_hash,
  stateRoot: r.state_root,
};
const tmp = OUT + ".tmp";
writeFileSync(tmp, JSON.stringify(snapshot, null, 2) + "\n");
renameSync(tmp, OUT);
console.log(`chain-snapshot: wrote height ${r.height} (round ${r.round}, tx_count ${r.tx_count})`);
