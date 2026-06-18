/**
 * Transaction builders for NOVAI transaction types.
 *
 * Each builder constructs a fully signed TxV1 ready for submission.
 */

import * as nacl from "tweetnacl";
import { addressFromPubkey, tweetnaclSecretKey } from "./keys";
import { encodeTxV1Unsigned } from "./encoding";
import {
  Keypair,
  TxV1,
  AutonomyMode,
  SignalType,
  MemoryObjectType,
  Capabilities,
} from "./types";

const TX_DOMAIN = Buffer.from("NOVAI_TX_V1");

/** Encode capabilities to a single byte. */
function capabilitiesToByte(caps: Capabilities): number {
  let flags = 0;
  if (caps.readPublicChain) flags |= 1 << 0;
  if (caps.readMemoryObjects) flags |= 1 << 1;
  if (caps.emitProposals) flags |= 1 << 2;
  if (caps.requestExecution) flags |= 1 << 3;
  if (caps.readNnpxDerived) flags |= 1 << 4;
  return flags;
}

/** Sign a TxV1 in place and return it. */
function signTx(kp: Keypair, tx: TxV1): TxV1 {
  const unsigned = encodeTxV1Unsigned(tx);
  const toSign = new Uint8Array(TX_DOMAIN.length + unsigned.length);
  toSign.set(TX_DOMAIN, 0);
  toSign.set(unsigned, TX_DOMAIN.length);

  const sk = tweetnaclSecretKey(kp);
  const sig = nacl.sign.detached(toSign, sk);
  tx.sig = new Uint8Array(sig);
  return tx;
}

/** Build an unsigned TxV1 shell. */
function buildUnsigned(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  payload: Uint8Array
): TxV1 {
  return {
    version: 1,
    from: kp.address,
    pubkey: kp.publicKey,
    nonce,
    fee,
    payload,
    sig: new Uint8Array(64),
  };
}

/** Build and sign a transaction. */
function buildSigned(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  payload: Uint8Array
): TxV1 {
  const tx = buildUnsigned(kp, nonce, fee, payload);
  return signTx(kp, tx);
}

// ============================================================================
// Type 1: Transfer
// ============================================================================

/** Build a transfer transaction. */
export function transfer(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  to: Uint8Array,
  amount: bigint
): TxV1 {
  const payload = new Uint8Array(41);
  const view = new DataView(payload.buffer);
  payload[0] = 1;
  payload.set(to, 1);
  view.setBigUint64(33, amount, false); // big-endian
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 2: Signal Commitment
// ============================================================================

/**
 * Build a signal commitment transaction (tx payload type 2).
 *
 * Envelope layout: `[0x02][signal_hash:32][signal_type:1][issuer_entity_id:32]`
 * (66 bytes). Signal types that carry an inline payload tail append their
 * type-specific `extras` after the envelope; build the tail with the matching
 * `build*Extras` helper from `./signals`. Types 0-6 carry no extras, so callers
 * for those omit the argument (preserving the 66-byte payload).
 */
export function signalCommitment(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  signalHash: Uint8Array,
  signalType: SignalType,
  issuerEntityId: Uint8Array,
  extras?: Uint8Array
): TxV1 {
  const extrasLen = extras?.length ?? 0;
  const payload = new Uint8Array(66 + extrasLen);
  payload[0] = 2;
  payload.set(signalHash, 1);
  payload[33] = signalType;
  payload.set(issuerEntityId, 34);
  if (extras && extras.length > 0) {
    payload.set(extras, 66);
  }
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 3: Create Memory Object
// ============================================================================

/** Build a create-memory-object transaction. */
export function createMemory(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  objectType: MemoryObjectType,
  data: Uint8Array
): TxV1 {
  const payload = new Uint8Array(6 + data.length);
  const view = new DataView(payload.buffer);
  payload[0] = 3;
  payload[1] = objectType;
  view.setUint32(2, data.length, false); // big-endian
  payload.set(data, 6);
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 4: Update Memory Object
// ============================================================================

/** Build an update-memory-object transaction. */
export function updateMemory(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  objectId: Uint8Array,
  data: Uint8Array
): TxV1 {
  const payload = new Uint8Array(37 + data.length);
  const view = new DataView(payload.buffer);
  payload[0] = 4;
  payload.set(objectId, 1);
  view.setUint32(33, data.length, false); // big-endian
  payload.set(data, 37);
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 5: Delete Memory Object
// ============================================================================

/** Build a delete-memory-object transaction. */
export function deleteMemory(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  objectId: Uint8Array
): TxV1 {
  const payload = new Uint8Array(33);
  payload[0] = 5;
  payload.set(objectId, 1);
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 6: Submit Governance Proposal
// ============================================================================

/** Build a submit-governance-proposal transaction. */
export function submitProposal(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  proposalType: number,
  gateId: Uint8Array,
  proposalData: Uint8Array
): TxV1 {
  const payload = new Uint8Array(38 + proposalData.length);
  const view = new DataView(payload.buffer);
  payload[0] = 6;
  payload[1] = proposalType;
  payload.set(gateId, 2);
  view.setUint32(34, proposalData.length, false); // big-endian
  payload.set(proposalData, 38);
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 7: Execute Governance Proposal
// ============================================================================

/** Build an execute-governance-proposal transaction. */
export function executeProposal(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  proposalId: Uint8Array
): TxV1 {
  const payload = new Uint8Array(33);
  payload[0] = 7;
  payload.set(proposalId, 1);
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 8: Register AI Entity
// ============================================================================

/** Build a register-AI-entity transaction (no entity key). */
export function registerAiEntity(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  codeHash: Uint8Array,
  autonomy: AutonomyMode,
  capabilities: Capabilities,
  initialBalance: bigint
): TxV1 {
  const payload = new Uint8Array(51);
  const view = new DataView(payload.buffer);
  payload[0] = 8;
  payload.set(codeHash, 1);
  payload[33] = autonomy;
  payload[34] = capabilitiesToByte(capabilities);
  view.setBigUint64(35, initialBalance >> 64n, false); // high 8 bytes
  view.setBigUint64(43, initialBalance & 0xFFFFFFFFFFFFFFFFn, false); // low 8 bytes
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 9: Credit AI Entity
// ============================================================================

/** Build a credit-AI-entity transaction. */
export function creditAiEntity(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  entityId: Uint8Array,
  amount: bigint
): TxV1 {
  const payload = new Uint8Array(49);
  const view = new DataView(payload.buffer);
  payload[0] = 9;
  payload.set(entityId, 1);
  view.setBigUint64(33, amount >> 64n, false); // high 8 bytes
  view.setBigUint64(41, amount & 0xFFFFFFFFFFFFFFFFn, false); // low 8 bytes
  return buildSigned(kp, nonce, fee, payload);
}

// ============================================================================
// Type 10: Register AI Entity with Key
// ============================================================================

/** Build a register-AI-entity-with-key transaction. */
export function registerAiEntityWithKey(
  kp: Keypair,
  nonce: bigint,
  fee: bigint,
  codeHash: Uint8Array,
  entityPubkey: Uint8Array,
  autonomy: AutonomyMode,
  capabilities: Capabilities,
  initialBalance: bigint
): TxV1 {
  const payload = new Uint8Array(83);
  const view = new DataView(payload.buffer);
  payload[0] = 10;
  payload.set(codeHash, 1);
  payload.set(entityPubkey, 33);
  payload[65] = autonomy;
  payload[66] = capabilitiesToByte(capabilities);
  view.setBigUint64(67, initialBalance >> 64n, false); // high 8 bytes
  view.setBigUint64(75, initialBalance & 0xFFFFFFFFFFFFFFFFn, false); // low 8 bytes
  return buildSigned(kp, nonce, fee, payload);
}
