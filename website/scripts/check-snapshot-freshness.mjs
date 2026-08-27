#!/usr/bin/env node
// Snapshot freshness gate: is the committed chain snapshot still inside the
// node's retention window?
//
// Why this exists: the snapshot is the prerendered and no-JS state of the live
// panels. A node keeps only the last PRUNE_RETAIN_BLOCKS blocks, so once the
// tip runs far enough ahead, every height in the committed snapshot answers
// null. That is not cosmetic staleness: it makes the fallback state reference
// blocks the chain will no longer serve.
//
// Threshold: read from crates/consensus/src/lib.rs rather than hand-typed, so
// the gate tracks the protocol constant instead of drifting from it.
//
// Deliberately NOT wired into prebuild. It makes a network call, and builds are
// hermetic and network-free by decision. Run it before a deploy:
//     npm run snapshot:check    (verify)
//     npm run snapshot          (refresh, then commit the file)

import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const SNAPSHOT = join(SCRIPT_DIR, "..", "src", "data", "chain-snapshot.json");
const CONSENSUS = join(SCRIPT_DIR, "..", "..", "crates", "consensus", "src", "lib.rs");
const URL = process.env.SNAPSHOT_RPC_URL || "https://rpc.novai.network";

function fail(msg) {
  console.error(`snapshot-freshness: FAIL: ${msg}`);
  process.exit(1);
}

// 1. The retention window, derived from source.
let retain;
try {
  const src = readFileSync(CONSENSUS, "utf8");
  const m = src.match(/^pub const PRUNE_RETAIN_BLOCKS: u64 = ([0-9_]+);$/m);
  if (!m) fail(`PRUNE_RETAIN_BLOCKS not found in ${CONSENSUS}; the counting method needs re-verification`);
  retain = Number(m[1].replace(/_/g, ""));
  if (!Number.isInteger(retain) || retain <= 0) fail(`PRUNE_RETAIN_BLOCKS parsed as ${m[1]}, which is not a positive integer`);
} catch (err) {
  if (err?.code === "ENOENT") fail(`cannot read ${CONSENSUS}`);
  throw err;
}

// 2. The committed snapshot.
let snap;
try {
  snap = JSON.parse(readFileSync(SNAPSHOT, "utf8"));
} catch {
  fail(`cannot read or parse ${SNAPSHOT}`);
}
if (typeof snap.height !== "number") fail("committed snapshot has no numeric height");

// 3. The live tip.
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
if (!body.result || typeof body.result.height !== "number") fail("result missing or malformed");

const tip = body.result.height;
const gap = tip - snap.height;

console.log(`snapshot-freshness: committed height ${snap.height}, live tip ${tip}`);
console.log(`snapshot-freshness: gap ${gap} block(s), retention window ${retain} (PRUNE_RETAIN_BLOCKS)`);

if (gap > retain) {
  fail(
    `the committed snapshot is ${gap} blocks behind the tip, past the ${retain} block retention window. ` +
      `Every height in it now answers null. Run npm run snapshot and commit the result.`
  );
}
if (gap < 0) {
  fail(
    `the committed snapshot is ${-gap} blocks AHEAD of the live tip. The chain has been reset or this is a ` +
      `different network. Run npm run snapshot and commit the result.`
  );
}
console.log("snapshot-freshness: ok (committed snapshot is inside the retention window)");
