/**
 * Canonical binary encoding for NOVAI TxV1 transactions.
 *
 * Field order is consensus-critical:
 * [version:1][from:32][pubkey:32][nonce:8 LE][fee:8 LE][payload_len:4 LE][payload:N]
 *
 * Signed encoding appends the 64-byte signature.
 */

import { createHash } from "blake3";
import { TxV1 } from "./types";

/** Total overhead for a signed TxV1 (everything except payload). */
export const TX_V1_OVERHEAD = 149;

/**
 * Encode the unsigned portion of a TxV1 transaction.
 *
 * This is the canonical encoding used for signing and txid computation.
 */
export function encodeTxV1Unsigned(tx: TxV1): Uint8Array {
  const size = 1 + 32 + 32 + 8 + 8 + 4 + tx.payload.length;
  const buf = new ArrayBuffer(size);
  const out = new Uint8Array(buf);
  const view = new DataView(buf);
  let offset = 0;

  // version (1 byte)
  view.setUint8(offset, tx.version);
  offset += 1;

  // from (32 bytes)
  out.set(tx.from, offset);
  offset += 32;

  // pubkey (32 bytes)
  out.set(tx.pubkey, offset);
  offset += 32;

  // nonce (8 bytes, little-endian u64)
  view.setBigUint64(offset, tx.nonce, true);
  offset += 8;

  // fee (8 bytes, little-endian u64)
  view.setBigUint64(offset, tx.fee, true);
  offset += 8;

  // payload_len (4 bytes, little-endian u32)
  view.setUint32(offset, tx.payload.length, true);
  offset += 4;

  // payload (variable)
  out.set(tx.payload, offset);

  return out;
}

/**
 * Encode a signed TxV1 transaction (unsigned bytes + 64-byte signature).
 */
export function encodeTxV1Signed(tx: TxV1): Uint8Array {
  const unsigned = encodeTxV1Unsigned(tx);
  const out = new Uint8Array(unsigned.length + 64);
  out.set(unsigned, 0);
  out.set(tx.sig, unsigned.length);
  return out;
}

/**
 * Compute the transaction ID: blake3(unsigned bytes).
 */
export function txidV1(tx: TxV1): Uint8Array {
  const unsigned = encodeTxV1Unsigned(tx);
  const hasher = createHash();
  hasher.update(unsigned);
  return new Uint8Array(hasher.digest());
}

/**
 * Compute the AI entity ID from code hash and creator address.
 *
 * `entity_id = blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)`
 */
export function computeEntityId(
  codeHash: Uint8Array,
  creator: Uint8Array
): Uint8Array {
  const domain = Buffer.from("NOVAI_AI_ENTITY_ID_V1");
  const hasher = createHash();
  hasher.update(domain);
  hasher.update(codeHash);
  hasher.update(creator);
  return new Uint8Array(hasher.digest());
}

/** Encode a hex string to bytes. */
export function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) {
    throw new Error("Hex string must have even length");
  }
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.substring(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/** Encode bytes to hex string. */
export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
