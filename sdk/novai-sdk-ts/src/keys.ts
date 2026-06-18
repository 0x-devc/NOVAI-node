/**
 * Key generation, loading, and address derivation.
 *
 * Uses tweetnacl for Ed25519 and blake3 for address hashing.
 */

import * as nacl from "tweetnacl";
import { createHash } from "blake3";
import { Keypair } from "./types";
import { channelStateSigningBytes, NOVAI_CHANNEL_CHAIN_ID } from "./signals";

const ADDRESS_DOMAIN = Buffer.from("NOVAI_ADDRESS_V1");

/**
 * Derive the canonical 32-byte NOVAI address from an Ed25519 public key.
 *
 * `address = blake3("NOVAI_ADDRESS_V1" || pubkey)`
 */
export function addressFromPubkey(pubkey: Uint8Array): Uint8Array {
  const hasher = createHash();
  hasher.update(ADDRESS_DOMAIN);
  hasher.update(pubkey);
  return new Uint8Array(hasher.digest());
}

/**
 * Generate a new random Ed25519 keypair with its NOVAI address.
 */
export function generateKeypair(): Keypair {
  const kp = nacl.sign.keyPair();
  // tweetnacl generates a random keypair; extract the 32-byte seed
  const seed = kp.secretKey.slice(0, 32);
  const address = addressFromPubkey(kp.publicKey);
  return {
    seed: new Uint8Array(seed),
    publicKey: new Uint8Array(kp.publicKey),
    address,
  };
}

/**
 * Create a keypair from a 32-byte seed.
 *
 * This is the same seed format used by NOVAI key files (raw 32 bytes).
 */
export function keypairFromSeed(seed: Uint8Array): Keypair {
  if (seed.length !== 32) {
    throw new Error(`Seed must be 32 bytes, got ${seed.length}`);
  }
  const kp = nacl.sign.keyPair.fromSeed(seed);
  const address = addressFromPubkey(kp.publicKey);
  return {
    seed: new Uint8Array(seed),
    publicKey: new Uint8Array(kp.publicKey),
    address,
  };
}

/**
 * Load a keypair from a 32-byte key file (Node.js only).
 *
 * Key files contain a raw 32-byte Ed25519 seed with no encoding.
 */
export function loadKeyFile(path: string): Keypair {
  // Dynamic require to avoid breaking browser environments
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  let fs: typeof import("fs");
  try {
    fs = require("fs");
  } catch {
    throw new Error("loadKeyFile is only available in Node.js environments");
  }
  const bytes = fs.readFileSync(path);
  if (bytes.length !== 32) {
    throw new Error(`Key file must be 32 bytes, got ${bytes.length}`);
  }
  return keypairFromSeed(new Uint8Array(bytes));
}

/**
 * Get the 64-byte tweetnacl secret key from a keypair (for signing).
 *
 * tweetnacl expects a 64-byte key (seed || pubkey) for detached signing.
 * This is an internal helper.
 */
export function tweetnaclSecretKey(kp: Keypair): Uint8Array {
  const sk = new Uint8Array(64);
  sk.set(kp.seed, 0);
  sk.set(kp.publicKey, 32);
  return sk;
}

const SLA_ACCEPT_SIGNAL_HASH_DOMAIN = Buffer.from("novai-sla-accept-v1");

/**
 * Derive the client-side content-addressed signal hash for an SlaAccept (type 18).
 *
 * `signal_hash = blake3("novai-sla-accept-v1" || sla_object_id || buyer_entity_id)`
 * (plain blake3; see tools/novai-cli/src/commands/sla.rs:162-166). The chain does
 * NOT validate this hash (the SlaAccept handler reads sla_object_id/buyer from the
 * extras tail), so it affects off-chain indexing and CLI matching only, not
 * on-chain semantics.
 */
export function deriveSlaAcceptSignalHash(
  slaObjectId: Uint8Array,
  buyerEntityId: Uint8Array
): Uint8Array {
  if (slaObjectId.length !== 32) {
    throw new RangeError(
      `slaObjectId must be 32 bytes, got ${slaObjectId.length}`
    );
  }
  if (buyerEntityId.length !== 32) {
    throw new RangeError(
      `buyerEntityId must be 32 bytes, got ${buyerEntityId.length}`
    );
  }
  const hasher = createHash();
  hasher.update(SLA_ACCEPT_SIGNAL_HASH_DOMAIN);
  hasher.update(slaObjectId);
  hasher.update(buyerEntityId);
  return new Uint8Array(hasher.digest());
}

/**
 * Sign a PaymentChannel off-chain state update, producing a ChannelClose
 * sig_a/sig_b. Returns the raw 64-byte ed25519 signature over
 * `channelStateSigningBytes(...)`, the same 167-byte message the chain verifies.
 * Each party signs independently with their own key.
 */
export function signChannelState(
  kp: Keypair,
  channelObjectId: Uint8Array,
  partyA: Uint8Array,
  partyB: Uint8Array,
  nonce: bigint,
  balanceA: bigint,
  balanceB: bigint,
  isFinal: boolean,
  chainId: bigint = NOVAI_CHANNEL_CHAIN_ID
): Uint8Array {
  const msg = channelStateSigningBytes(
    channelObjectId,
    partyA,
    partyB,
    nonce,
    balanceA,
    balanceB,
    isFinal,
    chainId
  );
  const sk = tweetnaclSecretKey(kp);
  return new Uint8Array(nacl.sign.detached(msg, sk));
}

const ORACLE_ANCHOR_SIGNAL_HASH_DOMAIN = Buffer.from("novai-oracle-anchor-v1");

/**
 * Derive the client-side content-addressed signal hash for an OracleAnchor (type 22).
 *
 * `signal_hash = blake3("novai-oracle-anchor-v1" || issuer || data_hash ||
 * external_timestamp_be:8 || source_hash || tag_len_be:u32:4 || data_tag)`
 * (plain blake3; see tools/novai-cli/src/commands/oracle.rs). The chain does NOT
 * recompute this; it uses the envelope signal_hash as the opaque storage/replay
 * key, so this is a content-addressing convention (off-chain consistency only),
 * not consensus-validated. Two traps: `tag_len` is a u32 big-endian here (4
 * bytes), whereas the OracleAnchor extras tail encodes the same length as a
 * single byte; and `issuerEntityId` feeds the hash but is absent from the tail.
 */
export function deriveOracleAnchorSignalHash(
  issuerEntityId: Uint8Array,
  dataHash: Uint8Array,
  externalTimestamp: bigint,
  sourceHash: Uint8Array | null,
  dataTag: Uint8Array
): Uint8Array {
  if (issuerEntityId.length !== 32) {
    throw new RangeError(`issuerEntityId must be 32 bytes, got ${issuerEntityId.length}`);
  }
  if (dataHash.length !== 32) {
    throw new RangeError(`dataHash must be 32 bytes, got ${dataHash.length}`);
  }
  const source = sourceHash ?? new Uint8Array(32);
  if (source.length !== 32) {
    throw new RangeError(`sourceHash must be 32 bytes, got ${source.length}`);
  }
  if (externalTimestamp < 0n || externalTimestamp >= 1n << 64n) {
    throw new RangeError(`externalTimestamp must fit in u64, got ${externalTimestamp}`);
  }
  if (dataTag.length < 1 || dataTag.length > 32) {
    throw new RangeError(`dataTag must be 1..=32 bytes, got ${dataTag.length}`);
  }
  const ts = new Uint8Array(8);
  new DataView(ts.buffer).setBigUint64(0, externalTimestamp, false); // u64 big-endian
  const tagLen = new Uint8Array(4);
  new DataView(tagLen.buffer).setUint32(0, dataTag.length, false); // u32 big-endian
  const hasher = createHash();
  hasher.update(ORACLE_ANCHOR_SIGNAL_HASH_DOMAIN);
  hasher.update(issuerEntityId);
  hasher.update(dataHash);
  hasher.update(ts);
  hasher.update(source);
  hasher.update(tagLen);
  hasher.update(dataTag);
  return new Uint8Array(hasher.digest());
}
