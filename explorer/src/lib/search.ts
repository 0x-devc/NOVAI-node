/**
 * Heuristic router for the global search box.
 *
 * Pure decimal              → block height
 * 64 hex chars              → could be block hash, txid, address, or entity_id.
 *                             We probe in order: block hash → tx → account.
 * Anything else             → null (caller shows "not recognized").
 */

import { rpc } from "./rpc";

export type SearchHit =
  | { kind: "block"; height: number }
  | { kind: "tx"; txid: string }
  | { kind: "address"; address: string };

export async function resolveSearch(input: string): Promise<SearchHit | null> {
  const q = input.trim().toLowerCase().replace(/^0x/, "");

  if (/^\d+$/.test(q)) {
    const height = Number(q);
    if (Number.isSafeInteger(height) && height >= 0) {
      return { kind: "block", height };
    }
    return null;
  }

  if (/^[0-9a-f]{64}$/.test(q)) {
    // Try block hash first — cheapest disambiguation.
    try {
      const block = await rpc.getBlockByHash(q);
      if (block) return { kind: "block", height: block.height };
    } catch {
      /* fall through */
    }

    // Then tx receipt.
    try {
      const tx = await rpc.getTransaction(q);
      if (tx) return { kind: "tx", txid: q };
    } catch {
      /* fall through */
    }

    // Default to account/entity address. The Account page itself
    // also probes getAiEntity, so this catches both.
    return { kind: "address", address: q };
  }

  return null;
}
