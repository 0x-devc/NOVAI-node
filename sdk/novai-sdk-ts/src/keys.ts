/**
 * Key generation, loading, and address derivation.
 *
 * Uses tweetnacl for Ed25519 and blake3 for address hashing.
 */

import * as nacl from "tweetnacl";
import { createHash } from "blake3";
import { Keypair } from "./types";

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
