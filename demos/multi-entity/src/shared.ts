/**
 * Shared bootstrap + helpers for both bots.
 *
 * Each bot's identity (creator seed, entity seed, entity_id) is persisted
 * to its own JSON file so restarts re-use the same on-chain entity.
 */

import * as fs from "fs";
import { createHash } from "blake3";
import {
  AutonomyMode,
  bytesToHex,
  computeEntityId,
  generateKeypair,
  hexToBytes,
  keypairFromSeed,
  Keypair,
  NovaiClient,
  registerAiEntityWithKey,
} from "@novai/sdk";

export interface BotState {
  creatorSeedHex: string;
  entitySeedHex: string;
  entityIdHex: string;
}

export const RPC_URL = process.env.NOVAI_RPC_URL ?? "http://localhost:3030";
export const REGISTER_FEE = 5_000n;
export const ENTITY_INITIAL_BALANCE = 500_000n;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

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

export function commitmentHash(input: string): Uint8Array {
  const h = createHash();
  h.update(Buffer.from(input, "utf-8"));
  return new Uint8Array(h.digest());
}

export function ts(): string {
  return new Date().toISOString().slice(11, 19);
}

/**
 * Bootstrap a bot: load or generate keys, faucet + register on chain if fresh.
 *
 * `codeHashByte` differentiates the entities — passing the same byte twice
 * with different creators is fine (creator address is part of compute_id),
 * but using a distinct byte makes the entity ids visibly different in logs.
 */
export async function bootstrap(
  client: NovaiClient,
  statePath: string,
  codeHashByte: number,
  label: string,
): Promise<{ state: BotState; entity: Keypair }> {
  const existing = loadState(statePath);
  if (existing) {
    console.log(
      `[${ts()}] [${label}] loaded existing state from ${statePath}`,
    );
    const entity = keypairFromSeed(hexToBytes(existing.entitySeedHex));
    return { state: existing, entity };
  }

  console.log(`[${ts()}] [${label}] first run — registering on chain`);

  const creator = generateKeypair();
  const entity = generateKeypair();
  const codeHash = new Uint8Array(32).fill(codeHashByte);

  await client.faucet(bytesToHex(creator.address));
  await sleep(1500);

  const nonce = await client.getNonce(bytesToHex(creator.address));
  const tx = registerAiEntityWithKey(
    creator,
    nonce,
    REGISTER_FEE,
    codeHash,
    entity.publicKey,
    AutonomyMode.Gated,
    {
      readPublicChain: true,
      readMemoryObjects: true,
      emitProposals: true,
    },
    ENTITY_INITIAL_BALANCE,
  );
  await client.submitTx(tx);
  await sleep(2000);

  const entityId = computeEntityId(codeHash, creator.address);
  const state: BotState = {
    creatorSeedHex: bytesToHex(creator.seed),
    entitySeedHex: bytesToHex(entity.seed),
    entityIdHex: bytesToHex(entityId),
  };
  saveState(statePath, state);
  console.log(
    `[${ts()}] [${label}] registered: entity ${state.entityIdHex.slice(0, 16)}…`,
  );
  return { state, entity };
}

/** Look up the current nonce of a bot's on-chain entity record. */
export async function entityNonce(
  client: NovaiClient,
  entityIdHex: string,
): Promise<bigint> {
  const e = await client.getAiEntity(entityIdHex);
  if (!e) throw new Error(`entity ${entityIdHex} not found`);
  return BigInt(e.nonce);
}
