/**
 * Bot state persistence — creator key, entity key, and entity_id.
 *
 * Keys are stored as hex-encoded 32-byte seeds inside a single JSON file
 * (bot-state.json, gitignored). On first run the file doesn't exist; we
 * generate fresh material, register the entity on chain, and write the
 * file. Subsequent runs load it and skip registration.
 */

import * as fs from "fs";

export interface BotState {
  /** Hex-encoded 32-byte creator seed. */
  creatorSeedHex: string;
  /** Hex-encoded 32-byte entity seed. */
  entitySeedHex: string;
  /** Hex entity_id (= blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator_addr)). */
  entityIdHex: string;
}

export function loadState(path: string): BotState | null {
  if (!fs.existsSync(path)) return null;
  try {
    return JSON.parse(fs.readFileSync(path, "utf-8")) as BotState;
  } catch {
    return null;
  }
}

export function saveState(path: string, state: BotState): void {
  fs.writeFileSync(path, JSON.stringify(state, null, 2));
  fs.chmodSync(path, 0o600);
}
