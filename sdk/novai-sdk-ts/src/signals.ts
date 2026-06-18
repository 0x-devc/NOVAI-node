/**
 * Per-signal-type "extras" tail builders for SignalCommitment (tx payload type 2).
 *
 * Each builder returns the inline payload tail that follows the 66-byte
 * SignalCommitment envelope produced by `signalCommitment(...)`. Pass the
 * returned bytes as that function's `extras` argument.
 *
 * Tail layouts and lengths are defined by the chain execution handler in
 * crates/execution/src/lib.rs. Integer fields inside tails are big-endian
 * (the TxV1 envelope is little-endian, but signal-payload integers are not).
 */

import { PaymentConditionKind, PaymentAttestationStatus } from "./types";

/** One past the maximum value of a u128 field. */
const U128_LIMIT = 1n << 128n;
/** One past the maximum value of a u64 field. */
const U64_LIMIT = 1n << 64n;
/** Inclusive bounds of a signed 16-bit integer field. */
const I16_MIN = -(1 << 15);
const I16_MAX = (1 << 15) - 1;

/**
 * Require an exact byte length (e.g. 32-byte entity ids). A short id would
 * otherwise be copied into a fixed-size tail and silently truncate it into the
 * wrong bytes with no error, so this guard is deliberate.
 */
function requireBytes(value: Uint8Array, len: number, field: string): void {
  if (value.length !== len) {
    throw new RangeError(`${field} must be ${len} bytes, got ${value.length}`);
  }
}

/** Require a value in the u8 range [0, 255]. */
function requireU8(value: number, field: string): void {
  if (!Number.isInteger(value) || value < 0 || value > 0xff) {
    throw new RangeError(`${field} must be a u8 (0..=255), got ${value}`);
  }
}

/** Require a value in the signed i16 range [-32768, 32767]. */
function requireI16(value: number, field: string): void {
  if (!Number.isInteger(value) || value < I16_MIN || value > I16_MAX) {
    throw new RangeError(
      `${field} must be an i16 (-32768..=32767), got ${value}`
    );
  }
}

/** Require a value in the u64 range [0, 2**64). */
function requireU64(value: bigint, field: string): void {
  if (value < 0n || value >= U64_LIMIT) {
    throw new RangeError(`${field} must fit in u64 (0 <= x < 2**64), got ${value}`);
  }
}

/**
 * Encode a non-negative bigint as a 16-byte big-endian u128.
 *
 * Mirrors the repo's existing u128 encoding (two big-endian 64-bit halves; see
 * `registerAiEntity` / `creditAiEntity` in ./tx) and the Python reference
 * `int.to_bytes(16, "big")`.
 */
function u128ToBeBytes(value: bigint, field: string): Uint8Array {
  if (value < 0n || value >= U128_LIMIT) {
    throw new RangeError(
      `${field} must fit in u128 (0 <= x < 2**128), got ${value}`
    );
  }
  const out = new Uint8Array(16);
  const view = new DataView(out.buffer);
  view.setBigUint64(0, value >> 64n, false); // high 8 bytes, big-endian
  view.setBigUint64(8, value & 0xffffffffffffffffn, false); // low 8 bytes, big-endian
  return out;
}

/**
 * ReputationUpdate (signal type 7) extras: 35 bytes.
 *
 * Layout: `[target_entity_id:32][event_type:1 u8][points_delta:2 i16 big-endian]`.
 * Total SignalCommitment payload is 101 (66 + 35), per REPUTATION_UPDATE_EXTRA_LEN
 * in the execution handler. `event_type` is a REP_EVENT_* discriminant; only its
 * u8 range is checked here (the chain validates the semantic range).
 */
export function buildReputationUpdateExtras(
  targetEntityId: Uint8Array,
  eventType: number,
  pointsDelta: number
): Uint8Array {
  requireBytes(targetEntityId, 32, "targetEntityId");
  requireU8(eventType, "eventType");
  requireI16(pointsDelta, "pointsDelta");
  const out = new Uint8Array(35);
  out.set(targetEntityId, 0);
  out[32] = eventType;
  new DataView(out.buffer).setInt16(33, pointsDelta, false); // i16, big-endian
  return out;
}

/**
 * SignalPurchase (signal type 8) extras: 41 bytes.
 *
 * Layout: `[seller_entity_id:32][purchased_signal_type:1 u8][max_price:8 u64 big-endian]`.
 * Total SignalCommitment payload is 107 (66 + 41), per SIGNAL_PURCHASE_EXTRA_LEN.
 * The chain additionally requires the purchased type to be listed in the
 * seller's on-chain SignalCatalog; that is not checked here.
 */
export function buildSignalPurchaseExtras(
  sellerEntityId: Uint8Array,
  purchasedSignalType: number,
  maxPrice: bigint
): Uint8Array {
  requireBytes(sellerEntityId, 32, "sellerEntityId");
  requireU8(purchasedSignalType, "purchasedSignalType");
  requireU64(maxPrice, "maxPrice");
  const out = new Uint8Array(41);
  out.set(sellerEntityId, 0);
  out[32] = purchasedSignalType;
  new DataView(out.buffer).setBigUint64(33, maxPrice, false); // u64, big-endian
  return out;
}

/**
 * StakeDeposit (signal type 9) extras: `[amount_be:16]` (16 bytes).
 *
 * Locks `amount` from the issuer's economic_balance into its stake_balance.
 * The full SignalCommitment payload is 82 bytes (66 envelope + 16 tail), per
 * SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_DEPOSIT_LEN in the execution handler.
 */
export function buildStakeDepositExtras(amount: bigint): Uint8Array {
  return u128ToBeBytes(amount, "amount");
}

/**
 * StakeWithdraw (signal type 10) extras: `[amount_be:16]` (16 bytes).
 *
 * Moves `amount` from the issuer's stake_balance back to its economic_balance.
 * The full SignalCommitment payload is 82 bytes (66 envelope + 16 tail), per
 * SIGNAL_COMMITMENT_PAYLOAD_V1_STAKE_WITHDRAW_LEN in the execution handler.
 */
export function buildStakeWithdrawExtras(amount: bigint): Uint8Array {
  return u128ToBeBytes(amount, "amount");
}

/**
 * StakeSlash (signal type 11) extras: 51 bytes.
 *
 * Layout:
 * `[target_entity_id:32][slash_amount:16 u128 big-endian][rep_event_type:1 u8][points_delta:2 i16 big-endian]`.
 * Total SignalCommitment payload is 117 (66 + 51), per STAKE_SLASH_EXTRA_LEN.
 */
export function buildStakeSlashExtras(
  targetEntityId: Uint8Array,
  slashAmount: bigint,
  repEventType: number,
  pointsDelta: number
): Uint8Array {
  requireBytes(targetEntityId, 32, "targetEntityId");
  requireU8(repEventType, "repEventType");
  requireI16(pointsDelta, "pointsDelta");
  const out = new Uint8Array(51);
  out.set(targetEntityId, 0);
  out.set(u128ToBeBytes(slashAmount, "slashAmount"), 32);
  out[48] = repEventType;
  new DataView(out.buffer).setInt16(49, pointsDelta, false); // i16, big-endian
  return out;
}

/**
 * CompositionCheck (signal type 12) extras: 34 bytes.
 *
 * Layout: `[target_entity_id:32][failed_dependency_idx:1 u8][failure_reason:1 u8]`.
 * Total SignalCommitment payload is 100 (66 + 34), per COMPOSITION_CHECK_EXTRA_LEN.
 */
export function buildCompositionCheckExtras(
  targetEntityId: Uint8Array,
  failedDependencyIdx: number,
  failureReason: number
): Uint8Array {
  requireBytes(targetEntityId, 32, "targetEntityId");
  requireU8(failedDependencyIdx, "failedDependencyIdx");
  requireU8(failureReason, "failureReason");
  const out = new Uint8Array(34);
  out.set(targetEntityId, 0);
  out[32] = failedDependencyIdx;
  out[33] = failureReason;
  return out;
}

/**
 * SubscriptionCreate (signal type 14) extras: 49 bytes.
 *
 * Layout:
 * `[producer_entity_id:32][covered_signal_type:1 u8][rate_per_block:8 u64 big-endian][duration_blocks:8 u64 big-endian]`.
 * Total SignalCommitment payload is 115 (66 + 49), per SUBSCRIPTION_CREATE_EXTRA_LEN.
 * The chain enforces `duration_blocks >= MIN_SUBSCRIPTION_DURATION` (a semantic
 * floor); that is not checked here, only the u64 wire range.
 */
export function buildSubscriptionCreateExtras(
  producerEntityId: Uint8Array,
  coveredSignalType: number,
  ratePerBlock: bigint,
  durationBlocks: bigint
): Uint8Array {
  requireBytes(producerEntityId, 32, "producerEntityId");
  requireU8(coveredSignalType, "coveredSignalType");
  requireU64(ratePerBlock, "ratePerBlock");
  requireU64(durationBlocks, "durationBlocks");
  const out = new Uint8Array(49);
  out.set(producerEntityId, 0);
  out[32] = coveredSignalType;
  const view = new DataView(out.buffer);
  view.setBigUint64(33, ratePerBlock, false); // u64, big-endian
  view.setBigUint64(41, durationBlocks, false); // u64, big-endian
  return out;
}

/**
 * SubscriptionCancel (signal type 15) extras: 32 bytes.
 *
 * Layout: `[subscription_id:32]` (the memory-object id of the Subscription
 * being cancelled). Total SignalCommitment payload is 98 (66 + 32), per
 * SUBSCRIPTION_CANCEL_EXTRA_LEN. Returns a copy so later mutation of the input
 * does not alias the tail.
 */
export function buildSubscriptionCancelExtras(
  subscriptionId: Uint8Array
): Uint8Array {
  requireBytes(subscriptionId, 32, "subscriptionId");
  return Uint8Array.from(subscriptionId);
}

/**
 * SlaAccept (signal type 18) extras: 64 bytes.
 *
 * Layout: `[sla_object_id:32][buyer_entity_id:32]`. Total SignalCommitment
 * payload is 130 (66 + 64), per SLA_ACCEPT_EXTRA_LEN. The envelope signal_hash
 * for this type is content-addressed via `deriveSlaAcceptSignalHash` in ./keys
 * (a client convention; the chain does not validate it).
 */
export function buildSlaAcceptExtras(
  slaObjectId: Uint8Array,
  buyerEntityId: Uint8Array
): Uint8Array {
  requireBytes(slaObjectId, 32, "slaObjectId");
  requireBytes(buyerEntityId, 32, "buyerEntityId");
  const out = new Uint8Array(64);
  out.set(slaObjectId, 0);
  out.set(buyerEntityId, 32);
  return out;
}

/**
 * ChannelAccept (signal type 19) extras: 64 bytes.
 *
 * Layout: `[channel_object_id:32][party_a_entity_id:32]`. Total 130, per
 * CHANNEL_ACCEPT_EXTRA_LEN.
 */
export function buildChannelAcceptExtras(
  channelObjectId: Uint8Array,
  partyAEntityId: Uint8Array
): Uint8Array {
  requireBytes(channelObjectId, 32, "channelObjectId");
  requireBytes(partyAEntityId, 32, "partyAEntityId");
  const out = new Uint8Array(64);
  out.set(channelObjectId, 0);
  out.set(partyAEntityId, 32);
  return out;
}

/**
 * ChannelClose (signal type 20) extras: 233 bytes.
 *
 * Layout:
 * `[channel_object_id:32][party_a_entity_id:32][nonce:8 u64 BE][balance_a:16 u128 BE][balance_b:16 u128 BE][is_final:1][sig_a:64][sig_b:64]`.
 * Total 299, per CHANNEL_CLOSE_EXTRA_LEN. `sig_a`/`sig_b` are ed25519 signatures
 * over `channelStateSigningBytes(...)`; produce them with `signChannelState`
 * in ./keys. The tail carries `party_a` but not `party_b`.
 */
export function buildChannelCloseExtras(
  channelObjectId: Uint8Array,
  partyAEntityId: Uint8Array,
  nonce: bigint,
  balanceA: bigint,
  balanceB: bigint,
  isFinal: boolean,
  sigA: Uint8Array,
  sigB: Uint8Array
): Uint8Array {
  requireBytes(channelObjectId, 32, "channelObjectId");
  requireBytes(partyAEntityId, 32, "partyAEntityId");
  requireU64(nonce, "nonce");
  requireBytes(sigA, 64, "sigA");
  requireBytes(sigB, 64, "sigB");
  const out = new Uint8Array(233);
  out.set(channelObjectId, 0);
  out.set(partyAEntityId, 32);
  const view = new DataView(out.buffer);
  view.setBigUint64(64, nonce, false); // nonce u64, big-endian
  out.set(u128ToBeBytes(balanceA, "balanceA"), 72);
  out.set(u128ToBeBytes(balanceB, "balanceB"), 88);
  out[104] = isFinal ? 1 : 0;
  out.set(sigA, 105);
  out.set(sigB, 169);
  return out;
}

/**
 * ChannelFinalize (signal type 21) extras: 64 bytes.
 *
 * Layout: `[channel_object_id:32][party_a_entity_id:32]`. Total 130, per
 * CHANNEL_FINALIZE_EXTRA_LEN.
 */
export function buildChannelFinalizeExtras(
  channelObjectId: Uint8Array,
  partyAEntityId: Uint8Array
): Uint8Array {
  requireBytes(channelObjectId, 32, "channelObjectId");
  requireBytes(partyAEntityId, 32, "partyAEntityId");
  const out = new Uint8Array(64);
  out.set(channelObjectId, 0);
  out.set(partyAEntityId, 32);
  return out;
}

/**
 * Numeric chain id bound into every PaymentChannel off-chain state signature.
 * Hardcoded to 1 in the chain for v1 (NOVAI_CHANNEL_CHAIN_ID in the execution
 * crate); exposed here as a parameter default for forward-compatibility.
 */
export const NOVAI_CHANNEL_CHAIN_ID = 1n;

/** Domain tag for the channel-state signing message ("NOVAI_CHANNEL_STATE_V1", 22 ascii bytes). */
const CHANNEL_STATE_DOMAIN = Uint8Array.from("NOVAI_CHANNEL_STATE_V1", (c) =>
  c.charCodeAt(0)
);

/**
 * Build the canonical 167-byte message both channel parties sign for a
 * ChannelClose state update. Consensus-critical: the chain verifies sig_a/sig_b
 * against exactly these bytes (crates/crypto/src/lib.rs:52-74).
 *
 * Layout (all integers big-endian):
 * `"NOVAI_CHANNEL_STATE_V1"(22) || chain_id:8 || channel_object_id:32 || party_a:32 || party_b:32 || nonce:8 || balance_a:16 || balance_b:16 || is_final:1`.
 * `party_b` is present here but NOT in the ChannelClose extras tail.
 */
export function channelStateSigningBytes(
  channelObjectId: Uint8Array,
  partyA: Uint8Array,
  partyB: Uint8Array,
  nonce: bigint,
  balanceA: bigint,
  balanceB: bigint,
  isFinal: boolean,
  chainId: bigint = NOVAI_CHANNEL_CHAIN_ID
): Uint8Array {
  requireBytes(channelObjectId, 32, "channelObjectId");
  requireBytes(partyA, 32, "partyA");
  requireBytes(partyB, 32, "partyB");
  requireU64(chainId, "chainId");
  requireU64(nonce, "nonce");
  const out = new Uint8Array(167);
  out.set(CHANNEL_STATE_DOMAIN, 0); // [0..22]
  const view = new DataView(out.buffer);
  view.setBigUint64(22, chainId, false); // chain_id u64, big-endian [22..30]
  out.set(channelObjectId, 30); // [30..62]
  out.set(partyA, 62); // [62..94]
  out.set(partyB, 94); // [94..126]
  view.setBigUint64(126, nonce, false); // nonce u64, big-endian [126..134]
  out.set(u128ToBeBytes(balanceA, "balanceA"), 134); // [134..150]
  out.set(u128ToBeBytes(balanceB, "balanceB"), 150); // [150..166]
  out[166] = isFinal ? 1 : 0; // [166]
  return out;
}

/** Max inline vk_bytes length in a v2 ProofSubmission (consensus-enforced; 8 KiB). */
export const PROOF_SUBMISSION_MAX_VK_BYTES = 8192;
/** Max proof_bytes length in a v2 ProofSubmission (consensus-enforced; 1 KiB). */
export const PROOF_SUBMISSION_MAX_PROOF_BYTES = 1024;

/**
 * ProofSubmission stub (signal type 13, proof_type 0) extras: 65 bytes.
 *
 * Layout: `[proof_type=0:1][code_hash:32][computation_hash:32]`. Total 131.
 * Development/smoke only; the stub verifier always accepts.
 */
export function buildProofSubmissionStubExtras(
  codeHash: Uint8Array,
  computationHash: Uint8Array
): Uint8Array {
  requireBytes(codeHash, 32, "codeHash");
  requireBytes(computationHash, 32, "computationHash");
  const out = new Uint8Array(65);
  out[0] = 0; // PROOF_TYPE_STUB
  out.set(codeHash, 1);
  out.set(computationHash, 33);
  return out;
}

/**
 * ProofSubmission inline-VK Groth16 (signal type 13, proof_type 1) extras (variable).
 *
 * Layout:
 * `[proof_type=1:1][code_hash:32][computation_hash:32][vk_len:4 u32 BE][vk_bytes][proof_len:4 u32 BE][proof_bytes]`.
 * Total 139 + vk_len + proof_len. `vk_bytes` <= 8192 and `proof_bytes` <= 1024
 * (the chain rejects larger with VerifyingKeyTooLarge / ProofBytesTooLarge).
 */
export function buildProofSubmissionGroth16Extras(
  codeHash: Uint8Array,
  computationHash: Uint8Array,
  vkBytes: Uint8Array,
  proofBytes: Uint8Array
): Uint8Array {
  requireBytes(codeHash, 32, "codeHash");
  requireBytes(computationHash, 32, "computationHash");
  if (vkBytes.length > PROOF_SUBMISSION_MAX_VK_BYTES) {
    throw new RangeError(
      `vkBytes exceeds PROOF_SUBMISSION_MAX_VK_BYTES (${PROOF_SUBMISSION_MAX_VK_BYTES}), got ${vkBytes.length}`
    );
  }
  if (proofBytes.length > PROOF_SUBMISSION_MAX_PROOF_BYTES) {
    throw new RangeError(
      `proofBytes exceeds PROOF_SUBMISSION_MAX_PROOF_BYTES (${PROOF_SUBMISSION_MAX_PROOF_BYTES}), got ${proofBytes.length}`
    );
  }
  const out = new Uint8Array(65 + 4 + vkBytes.length + 4 + proofBytes.length);
  out[0] = 1; // PROOF_TYPE_GROTH16
  out.set(codeHash, 1);
  out.set(computationHash, 33);
  const view = new DataView(out.buffer);
  view.setUint32(65, vkBytes.length, false); // vk_len u32, big-endian
  out.set(vkBytes, 69);
  const proofLenOff = 69 + vkBytes.length;
  view.setUint32(proofLenOff, proofBytes.length, false); // proof_len u32, big-endian
  out.set(proofBytes, proofLenOff + 4);
  return out;
}

/**
 * ProofSubmission registered-VK Groth16 (signal type 13, proof_type 3) extras (variable).
 *
 * Layout:
 * `[proof_type=3:1][code_hash:32][computation_hash:32][vk_len=32:4 u32 BE][vk_id:32][proof_len:4 u32 BE][proof_bytes]`.
 * Total 171 + proof_len. `vk_id` is the 32-byte VkRegistration memory-object id;
 * the chain requires vk_len == 32 for this type (and rejects otherwise).
 * `proof_bytes` <= 1024.
 */
export function buildProofSubmissionGroth16RegisteredExtras(
  codeHash: Uint8Array,
  computationHash: Uint8Array,
  vkId: Uint8Array,
  proofBytes: Uint8Array
): Uint8Array {
  requireBytes(codeHash, 32, "codeHash");
  requireBytes(computationHash, 32, "computationHash");
  requireBytes(vkId, 32, "vkId");
  if (proofBytes.length > PROOF_SUBMISSION_MAX_PROOF_BYTES) {
    throw new RangeError(
      `proofBytes exceeds PROOF_SUBMISSION_MAX_PROOF_BYTES (${PROOF_SUBMISSION_MAX_PROOF_BYTES}), got ${proofBytes.length}`
    );
  }
  const out = new Uint8Array(65 + 4 + 32 + 4 + proofBytes.length);
  out[0] = 3; // PROOF_TYPE_GROTH16_REGISTERED
  out.set(codeHash, 1);
  out.set(computationHash, 33);
  const view = new DataView(out.buffer);
  view.setUint32(65, 32, false); // vk_len = 32 (registry handle), u32 big-endian
  out.set(vkId, 69);
  view.setUint32(101, proofBytes.length, false); // proof_len u32, big-endian
  out.set(proofBytes, 105);
  return out;
}

// ============================================================================
// Family 6: PaymentRequest (16), ServiceAttestation (17), OracleAnchor (22)
// ============================================================================

/** PaymentRequest conditional-execution marker byte (0xC1). */
export const PAYMENT_CONDITION_MARKER = 0xc1;
/** Minimum number of split recipients when a splits trailer is present. */
export const MIN_PAYMENT_SPLITS_WHEN_PRESENT = 2;
/** Maximum number of split recipients. */
export const MAX_PAYMENT_SPLITS = 8;
/** Basis-points denominator; all split shares must sum to this. */
export const BPS_DENOMINATOR = 10000;
/** Minimum OracleAnchor / condition data_tag length. */
export const ORACLE_ANCHOR_DATA_TAG_MIN_LEN = 1;
/** Maximum OracleAnchor / condition data_tag length. */
export const ORACLE_ANCHOR_DATA_TAG_MAX_LEN = 32;

function concatBytes(parts: readonly Uint8Array[]): Uint8Array {
  let total = 0;
  for (const p of parts) total += p.length;
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function toHexKey(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i++) s += b[i].toString(16).padStart(2, "0");
  return s;
}

/** One recipient share in a PaymentRequest splits trailer (Week 33). */
export interface PaymentSplit {
  /** 32-byte entity id receiving this share. */
  recipientEntityId: Uint8Array;
  /** u16 share in basis points (1..=10000); all shares must sum to 10000. */
  basisPoints: number;
}

/**
 * A PaymentRequest conditional-execution gate (Week 36). Construct with the
 * `paymentCondition*` factories; the operand fields present depend on `kind`.
 */
export interface PaymentCondition {
  readonly kind: PaymentConditionKind;
  readonly anchorSignalHash: Uint8Array;
  readonly expectedDataHash?: Uint8Array;
  readonly expectedTag?: Uint8Array;
}

/** Condition kind 1: the referenced oracle anchor must exist. */
export function paymentConditionAnchorExists(
  anchorSignalHash: Uint8Array
): PaymentCondition {
  return { kind: PaymentConditionKind.AnchorExists, anchorSignalHash };
}

/** Condition kind 2: the anchor's data_hash must equal expectedDataHash. */
export function paymentConditionAnchorDataHashEquals(
  anchorSignalHash: Uint8Array,
  expectedDataHash: Uint8Array
): PaymentCondition {
  return {
    kind: PaymentConditionKind.AnchorDataHashEquals,
    anchorSignalHash,
    expectedDataHash,
  };
}

/** Condition kind 3: the anchor's data_tag must equal expectedTag (1..=32 bytes). */
export function paymentConditionAnchorTagEquals(
  anchorSignalHash: Uint8Array,
  expectedTag: Uint8Array
): PaymentCondition {
  return {
    kind: PaymentConditionKind.AnchorTagEquals,
    anchorSignalHash,
    expectedTag,
  };
}

/** Condition kind 4: the anchor's expiry_height must be 0 or >= current height. */
export function paymentConditionAnchorNotExpired(
  anchorSignalHash: Uint8Array
): PaymentCondition {
  return { kind: PaymentConditionKind.AnchorNotExpired, anchorSignalHash };
}

/** Encode a condition body (kind + operands), WITHOUT the 0xC1 marker. */
function encodePaymentCondition(c: PaymentCondition): Uint8Array {
  requireBytes(c.anchorSignalHash, 32, "anchorSignalHash");
  switch (c.kind) {
    case PaymentConditionKind.AnchorExists:
    case PaymentConditionKind.AnchorNotExpired: {
      const out = new Uint8Array(1 + 32);
      out[0] = c.kind;
      out.set(c.anchorSignalHash, 1);
      return out;
    }
    case PaymentConditionKind.AnchorDataHashEquals: {
      if (!c.expectedDataHash) {
        throw new RangeError("AnchorDataHashEquals requires expectedDataHash");
      }
      requireBytes(c.expectedDataHash, 32, "expectedDataHash");
      const out = new Uint8Array(1 + 32 + 32);
      out[0] = c.kind;
      out.set(c.anchorSignalHash, 1);
      out.set(c.expectedDataHash, 33);
      return out;
    }
    case PaymentConditionKind.AnchorTagEquals: {
      if (!c.expectedTag) {
        throw new RangeError("AnchorTagEquals requires expectedTag");
      }
      if (
        c.expectedTag.length < ORACLE_ANCHOR_DATA_TAG_MIN_LEN ||
        c.expectedTag.length > ORACLE_ANCHOR_DATA_TAG_MAX_LEN
      ) {
        throw new RangeError(
          `expectedTag must be ${ORACLE_ANCHOR_DATA_TAG_MIN_LEN}..=${ORACLE_ANCHOR_DATA_TAG_MAX_LEN} bytes, got ${c.expectedTag.length}`
        );
      }
      const out = new Uint8Array(1 + 32 + 1 + c.expectedTag.length);
      out[0] = c.kind;
      out.set(c.anchorSignalHash, 1);
      out[33] = c.expectedTag.length;
      out.set(c.expectedTag, 34);
      return out;
    }
    default:
      throw new RangeError(`unknown PaymentConditionKind ${c.kind}`);
  }
}

/** Encode a splits trailer: count byte + N * (recipient:32 || basis_points:2 BE). */
function encodePaymentSplits(splits: readonly PaymentSplit[]): Uint8Array {
  const out = new Uint8Array(1 + splits.length * 34);
  out[0] = splits.length;
  const view = new DataView(out.buffer);
  let off = 1;
  for (const s of splits) {
    requireBytes(s.recipientEntityId, 32, "recipientEntityId");
    if (
      !Number.isInteger(s.basisPoints) ||
      s.basisPoints < 0 ||
      s.basisPoints > 0xffff
    ) {
      throw new RangeError(`basisPoints must fit in u16 (0..=65535), got ${s.basisPoints}`);
    }
    out.set(s.recipientEntityId, off);
    view.setUint16(off + 32, s.basisPoints, false); // basis_points u16, big-endian
    off += 34;
  }
  return out;
}

/**
 * Opt-in semantic validation of a splits set, mirroring the Python/CLI checks.
 * NOT called automatically by the builder (the chain owns these economics):
 * count in [2,8], splits[0] == primary payee, no duplicate recipients, each
 * share in [1, 10000], and shares sum to exactly 10000.
 */
export function validatePaymentSplits(
  splits: readonly PaymentSplit[],
  payeeEntityId: Uint8Array
): void {
  if (
    splits.length < MIN_PAYMENT_SPLITS_WHEN_PRESENT ||
    splits.length > MAX_PAYMENT_SPLITS
  ) {
    throw new RangeError(
      `splits must have ${MIN_PAYMENT_SPLITS_WHEN_PRESENT}..=${MAX_PAYMENT_SPLITS} entries, got ${splits.length}`
    );
  }
  if (!bytesEqual(splits[0].recipientEntityId, payeeEntityId)) {
    throw new RangeError("splits[0].recipientEntityId must equal the primary payee");
  }
  const seen = new Set<string>();
  let totalBp = 0;
  for (const s of splits) {
    if (s.basisPoints < 1 || s.basisPoints > BPS_DENOMINATOR) {
      throw new RangeError(
        `basisPoints must be in [1, ${BPS_DENOMINATOR}], got ${s.basisPoints}`
      );
    }
    const key = toHexKey(s.recipientEntityId);
    if (seen.has(key)) {
      throw new RangeError(`duplicate split recipient ${key}`);
    }
    seen.add(key);
    totalBp += s.basisPoints;
  }
  if (totalBp !== BPS_DENOMINATOR) {
    throw new RangeError(`sum of basisPoints must equal ${BPS_DENOMINATOR}, got ${totalBp}`);
  }
}

/**
 * PaymentRequest (signal type 16) extras (variable).
 *
 * Base (112 bytes):
 * `[payee:32][amount:8 u64 BE][service_descriptor_hash:32][request_hash:32][max_block_height:8 u64 BE]`.
 * Optional trailers, appended condition-first then splits (matches the CLI):
 *   condition: `[0xC1][kind:1][operands]`
 *   splits: `[count:1 (2..=8)][recipient:32 || basis_points:2 BE] * count`
 * The builder enforces wire/structural constraints only; use
 * `validatePaymentSplits` for the chain-owned economic checks.
 */
export function buildPaymentRequestExtras(
  payeeEntityId: Uint8Array,
  amount: bigint,
  serviceDescriptorHash: Uint8Array,
  requestHash: Uint8Array,
  maxBlockHeight: bigint,
  options: { splits?: readonly PaymentSplit[]; condition?: PaymentCondition } = {}
): Uint8Array {
  requireBytes(payeeEntityId, 32, "payeeEntityId");
  requireU64(amount, "amount");
  requireBytes(serviceDescriptorHash, 32, "serviceDescriptorHash");
  requireBytes(requestHash, 32, "requestHash");
  requireU64(maxBlockHeight, "maxBlockHeight");

  const base = new Uint8Array(112);
  base.set(payeeEntityId, 0);
  const view = new DataView(base.buffer);
  view.setBigUint64(32, amount, false); // amount u64, big-endian [32..40]
  base.set(serviceDescriptorHash, 40);
  base.set(requestHash, 72);
  view.setBigUint64(104, maxBlockHeight, false); // [104..112]

  const parts: Uint8Array[] = [base];
  if (options.condition) {
    const body = encodePaymentCondition(options.condition);
    const cond = new Uint8Array(1 + body.length);
    cond[0] = PAYMENT_CONDITION_MARKER;
    cond.set(body, 1);
    parts.push(cond);
  }
  if (options.splits) {
    if (
      options.splits.length < MIN_PAYMENT_SPLITS_WHEN_PRESENT ||
      options.splits.length > MAX_PAYMENT_SPLITS
    ) {
      throw new RangeError(
        `splits must have ${MIN_PAYMENT_SPLITS_WHEN_PRESENT}..=${MAX_PAYMENT_SPLITS} entries when present, got ${options.splits.length}`
      );
    }
    parts.push(encodePaymentSplits(options.splits));
  }
  return concatBytes(parts);
}

/**
 * ServiceAttestation (signal type 17) extras: 65 bytes.
 *
 * Layout: `[payment_signal_hash:32][payee_entity_id:32][status:1]`. Total 131.
 * `status` must be 0 (Delivered) or 1 (Failed).
 */
export function buildServiceAttestationExtras(
  paymentSignalHash: Uint8Array,
  payeeEntityId: Uint8Array,
  status: PaymentAttestationStatus
): Uint8Array {
  requireBytes(paymentSignalHash, 32, "paymentSignalHash");
  requireBytes(payeeEntityId, 32, "payeeEntityId");
  if (
    status !== PaymentAttestationStatus.Delivered &&
    status !== PaymentAttestationStatus.Failed
  ) {
    throw new RangeError(`status must be 0 (Delivered) or 1 (Failed), got ${status}`);
  }
  const out = new Uint8Array(65);
  out.set(paymentSignalHash, 0);
  out.set(payeeEntityId, 32);
  out[64] = status;
  return out;
}

/**
 * OracleAnchor (signal type 22) extras: 82..=113 bytes.
 *
 * Layout:
 * `[data_hash:32][external_timestamp:8 u64 BE][source_hash:32][expiry_height:8 u64 BE][data_tag_len:1][data_tag:1..=32]`.
 * Total 148..=179. `sourceHash` null encodes 32 zero bytes. Note `data_tag_len`
 * here is a single byte; the signal-hash derivation encodes the same length as a
 * u32 big-endian instead (see `deriveOracleAnchorSignalHash` in ./keys).
 */
export function buildOracleAnchorExtras(
  dataHash: Uint8Array,
  externalTimestamp: bigint,
  sourceHash: Uint8Array | null,
  expiryHeight: bigint,
  dataTag: Uint8Array
): Uint8Array {
  requireBytes(dataHash, 32, "dataHash");
  requireU64(externalTimestamp, "externalTimestamp");
  requireU64(expiryHeight, "expiryHeight");
  if (
    dataTag.length < ORACLE_ANCHOR_DATA_TAG_MIN_LEN ||
    dataTag.length > ORACLE_ANCHOR_DATA_TAG_MAX_LEN
  ) {
    throw new RangeError(
      `dataTag must be ${ORACLE_ANCHOR_DATA_TAG_MIN_LEN}..=${ORACLE_ANCHOR_DATA_TAG_MAX_LEN} bytes, got ${dataTag.length}`
    );
  }
  const source = sourceHash ?? new Uint8Array(32);
  requireBytes(source, 32, "sourceHash");
  const out = new Uint8Array(81 + dataTag.length);
  out.set(dataHash, 0);
  const view = new DataView(out.buffer);
  view.setBigUint64(32, externalTimestamp, false); // [32..40]
  out.set(source, 40);
  view.setBigUint64(72, expiryHeight, false); // [72..80]
  out[80] = dataTag.length; // single byte (u32 BE in the hash input, not here)
  out.set(dataTag, 81);
  return out;
}
