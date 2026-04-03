/**
 * @novai/sdk — TypeScript SDK for interacting with NOVAI blockchain nodes.
 *
 * @example
 * ```typescript
 * import {
 *   generateKeypair,
 *   NovaiClient,
 *   transfer,
 *   registerAiEntityWithKey,
 *   createMemory,
 *   signalCommitment,
 *   computeEntityId,
 *   bytesToHex,
 *   AutonomyMode,
 *   SignalType,
 *   MemoryObjectType,
 * } from "@novai/sdk";
 *
 * const kp = generateKeypair();
 * const client = new NovaiClient("http://localhost:3030");
 *
 * // Get tokens
 * await client.faucet(bytesToHex(kp.address));
 *
 * // Build and submit a transfer
 * const nonce = await client.getNonce(bytesToHex(kp.address));
 * const tx = transfer(kp, nonce, 100n, recipientAddress, 1000n);
 * const txid = await client.submitTx(tx);
 * ```
 */

// Key management
export {
  generateKeypair,
  keypairFromSeed,
  loadKeyFile,
  addressFromPubkey,
} from "./keys";

// Transaction builders
export {
  transfer,
  signalCommitment,
  createMemory,
  updateMemory,
  deleteMemory,
  submitProposal,
  executeProposal,
  registerAiEntity,
  creditAiEntity,
  registerAiEntityWithKey,
} from "./tx";

// Encoding utilities
export {
  encodeTxV1Unsigned,
  encodeTxV1Signed,
  txidV1,
  computeEntityId,
  hexToBytes,
  bytesToHex,
  TX_V1_OVERHEAD,
} from "./encoding";

// RPC client
export { NovaiClient } from "./client";

// Types
export {
  Keypair,
  TxV1,
  Address,
  TxId,
  Hash32,
  SignatureBytes,
  AutonomyMode,
  SignalType,
  MemoryObjectType,
  Capabilities,
  AiEntityInfo,
  MemoryObjectInfo,
  SignalInfo,
} from "./types";
