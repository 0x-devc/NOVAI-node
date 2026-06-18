/**
 * Tests for the Family 4 derivation/signing helpers in ../src/keys:
 *   - deriveSlaAcceptSignalHash (type 18 envelope signal hash; client convention,
 *     NOT consensus-validated)
 *   - signChannelState (produces ChannelClose sig_a/sig_b)
 *
 * Oracle notes (honest coverage):
 *   - The SLA-hash golden vector was computed with the reference TS BLAKE3
 *     (proven below via the canonical empty-input vector) over inputs mapped
 *     byte-for-byte to the CLI source tools/novai-cli/src/commands/sla.rs:162-166.
 *     Python blake3 is NOT installed in this env and building the Rust CLI is out
 *     of scope, so this is anchored on reference-blake3 + source-mapping, not an
 *     externally executed hash. The SLA hash is non-consensus (the chain reads
 *     sla_object_id/buyer from the extras, never the envelope), so this is adequate.
 *   - signChannelState is exercised with a tweetnacl sign/verify round trip and a
 *     tamper test. ed25519 is a standard, so a tweetnacl signature verifies under
 *     any compliant verifier (including the chain's ed25519-dalek); cross-impl
 *     verification against the live chain is the same on-chain conformance gap.
 *
 * Imports ../src/keys (loads native blake3 + tweetnacl).
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "blake3";
import * as nacl from "tweetnacl";
import {
  deriveSlaAcceptSignalHash,
  signChannelState,
  generateKeypair,
  deriveOracleAnchorSignalHash,
} from "../src/keys";
import { channelStateSigningBytes } from "../src/signals";

const id = (b: number) => new Uint8Array(32).fill(b);
function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

describe("deriveSlaAcceptSignalHash (type 18, client convention)", () => {
  it("TS blake3 is reference BLAKE3 (canonical empty-input vector)", () => {
    assert.equal(
      Buffer.from(createHash().digest()).toString("hex"),
      "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
  });
  it("golden vector: blake3('novai-sla-accept-v1' || sla(0x77) || buyer(0x88))", () => {
    const h = deriveSlaAcceptSignalHash(id(0x77), id(0x88));
    assert.equal(h.length, 32);
    assert.deepEqual(
      h,
      fromHex("c76763e7de93ef8348da9180267e035586c848ad2c699e44278218518b6e2453")
    );
  });
  it("is deterministic and input-sensitive", () => {
    assert.deepEqual(
      deriveSlaAcceptSignalHash(id(0x77), id(0x88)),
      deriveSlaAcceptSignalHash(id(0x77), id(0x88))
    );
    assert.notDeepEqual(
      deriveSlaAcceptSignalHash(id(0x77), id(0x88)),
      deriveSlaAcceptSignalHash(id(0x78), id(0x88))
    );
    assert.notDeepEqual(
      deriveSlaAcceptSignalHash(id(0x77), id(0x88)),
      deriveSlaAcceptSignalHash(id(0x77), id(0x89))
    );
  });
  it("rejects non-32-byte inputs", () => {
    assert.throws(() => deriveSlaAcceptSignalHash(new Uint8Array(31), id(0x88)), RangeError);
    assert.throws(() => deriveSlaAcceptSignalHash(id(0x77), new Uint8Array(33)), RangeError);
  });
});

describe("signChannelState (ChannelClose sig_a/sig_b)", () => {
  const kp = generateKeypair();
  const cid = id(0xaa);
  const pa = id(0xbb);
  const pb = id(0xcc);

  it("produces a 64-byte sig that verifies over the 167-byte signing message", () => {
    const sigA = signChannelState(kp, cid, pa, pb, 42n, 1000n, 500n, false);
    assert.equal(sigA.length, 64);
    const msg = channelStateSigningBytes(cid, pa, pb, 42n, 1000n, 500n, false);
    assert.equal(nacl.sign.detached.verify(msg, sigA, kp.publicKey), true);
  });

  it("a tampered field (flipped balance) makes verification fail", () => {
    const sigA = signChannelState(kp, cid, pa, pb, 42n, 1000n, 500n, false);
    const tampered = channelStateSigningBytes(cid, pa, pb, 42n, 1001n, 500n, false);
    assert.equal(nacl.sign.detached.verify(tampered, sigA, kp.publicKey), false);
  });

  it("different chain_id produces a different signature", () => {
    const s1 = signChannelState(kp, cid, pa, pb, 1n, 0n, 0n, false);
    const s7 = signChannelState(kp, cid, pa, pb, 1n, 0n, 0n, false, 7n);
    assert.notDeepEqual(s1, s7);
  });
});

describe("deriveOracleAnchorSignalHash (type 22, client convention)", () => {
  // Golden vector via reference TS BLAKE3 (Python blake3 unavailable; flagged
  // like SLA). The chain does NOT recompute this hash (it is the opaque storage
  // / replay key), so this is content-addressing only, not consensus-validated.
  // Trap: tag_len is a u32 big-endian in the hash input (4 bytes), while the
  // OracleAnchor extras tail encodes the same length as one byte; and issuer
  // feeds the hash but is absent from the tail.
  it("golden vector blake3('novai-oracle-anchor-v1' || issuer(99) || dh(11) || ts(1234) || src(22) || tag_len_u32 || tag(aa))", () => {
    const h = deriveOracleAnchorSignalHash(id(0x99), id(0x11), 1234n, id(0x22), new Uint8Array([0xaa]));
    assert.equal(h.length, 32);
    assert.deepEqual(
      h,
      fromHex("161c1d6f3eb08fc74305db300b41277766b2bbe3680639a483574ece542ff872")
    );
  });
  it("null source equals explicit 32 zero bytes; issuer-sensitive", () => {
    const a = deriveOracleAnchorSignalHash(id(0x99), id(0x11), 1234n, null, new Uint8Array([0xaa]));
    const b = deriveOracleAnchorSignalHash(id(0x99), id(0x11), 1234n, new Uint8Array(32), new Uint8Array([0xaa]));
    assert.deepEqual(a, b);
    assert.notDeepEqual(
      a,
      deriveOracleAnchorSignalHash(id(0x98), id(0x11), 1234n, null, new Uint8Array([0xaa]))
    );
  });
  it("rejects non-32 inputs and out-of-range tag", () => {
    assert.throws(
      () => deriveOracleAnchorSignalHash(new Uint8Array(31), id(0x11), 1n, null, new Uint8Array([1])),
      RangeError
    );
    assert.throws(
      () => deriveOracleAnchorSignalHash(id(0x99), id(0x11), 1n, null, new Uint8Array(0)),
      RangeError
    );
  });
});
