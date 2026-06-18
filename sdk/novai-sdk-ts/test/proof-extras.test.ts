/**
 * Golden-vector tests for Family 5 signal extras: ProofSubmission (type 13),
 * three wired variants (stub=0, groth16 inline-VK=1, groth16 registered-VK=3).
 *
 * Ground truth (chain execution handler, crates/execution/src/lib.rs):
 *   constants 1240-1268 (65-byte v1 extra; caps 8192 vk / 1024 proof; v2 min)
 *   accepted proof_type set: is_supported_proof_type lib.rs:1840-1845 -> {0,1,3}
 *   decoder lib.rs:3237-3322: proof_type@66, code_hash[67..99], computation_hash[99..131],
 *     v2: vk_len u32 BE[131..135], vk_bytes, proof_len u32 BE, proof_bytes
 *     registered (3): vk_len MUST be 32, field is a VkRegistration handle
 *
 * Vectors produced by the real Python builders (signals/proof.py; no blake3/nacl).
 * Imports only ../src/signals.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildProofSubmissionStubExtras,
  buildProofSubmissionGroth16Extras,
  buildProofSubmissionGroth16RegisteredExtras,
  PROOF_SUBMISSION_MAX_VK_BYTES,
  PROOF_SUBMISSION_MAX_PROOF_BYTES,
} from "../src/signals";

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
const id = (b: number) => new Uint8Array(32).fill(b);
const C = id(0x11);
const CO = id(0x22);
const VKID = id(0x33);

describe("proof submission stub (type 13, proof_type 0): Python golden vector", () => {
  it("65-byte tail [0][code][comp]", () => {
    const e = buildProofSubmissionStubExtras(C, CO);
    assert.equal(e.length, 65);
    assert.deepEqual(e, fromHex("00" + "11".repeat(32) + "22".repeat(32)));
    assert.equal(e[0], 0); // proof_type
  });
});

describe("proof submission groth16 inline-VK (type 13, proof_type 1): Python golden vectors", () => {
  it("vk=5 proof=3 byte-for-byte", () => {
    const e = buildProofSubmissionGroth16Extras(
      C,
      CO,
      new Uint8Array(5).fill(0xab),
      new Uint8Array(3).fill(0xcd)
    );
    assert.equal(e.length, 81);
    assert.deepEqual(
      e,
      fromHex(
        "01" +
          "11".repeat(32) +
          "22".repeat(32) +
          "00000005" +
          "ab".repeat(5) +
          "00000003" +
          "cd".repeat(3)
      )
    );
  });
  it("u32 BE length prefixes at the right offsets", () => {
    const e = buildProofSubmissionGroth16Extras(
      C,
      CO,
      new Uint8Array(5).fill(0xab),
      new Uint8Array(3).fill(0xcd)
    );
    assert.deepEqual(e.slice(65, 69), fromHex("00000005")); // vk_len BE
    assert.deepEqual(e.slice(74, 78), fromHex("00000003")); // proof_len BE (at 69+5)
  });
  it("V2-min: empty vk + empty proof -> 73-byte tail", () => {
    const e = buildProofSubmissionGroth16Extras(C, CO, new Uint8Array(0), new Uint8Array(0));
    assert.equal(e.length, 73);
    assert.deepEqual(
      e,
      fromHex("01" + "11".repeat(32) + "22".repeat(32) + "00000000" + "00000000")
    );
  });
  it("cap boundary: vk=8192, proof=1024 accepted (tail length 9289, vk_len=00002000)", () => {
    const e = buildProofSubmissionGroth16Extras(
      C,
      CO,
      new Uint8Array(PROOF_SUBMISSION_MAX_VK_BYTES),
      new Uint8Array(PROOF_SUBMISSION_MAX_PROOF_BYTES)
    );
    assert.equal(e.length, 65 + 4 + 8192 + 4 + 1024);
    assert.deepEqual(e.slice(65, 69), fromHex("00002000")); // vk_len = 8192 BE
  });
  it("over-cap rejected (matches chain VerifyingKeyTooLarge / ProofBytesTooLarge)", () => {
    assert.throws(
      () => buildProofSubmissionGroth16Extras(C, CO, new Uint8Array(8193), new Uint8Array(0)),
      RangeError
    );
    assert.throws(
      () => buildProofSubmissionGroth16Extras(C, CO, new Uint8Array(0), new Uint8Array(1025)),
      RangeError
    );
  });
});

describe("proof submission registered-VK (type 13, proof_type 3): Python golden vector", () => {
  it("vk_id=32, proof=3 byte-for-byte; vk_len field is exactly 32", () => {
    const e = buildProofSubmissionGroth16RegisteredExtras(
      C,
      CO,
      VKID,
      new Uint8Array(3).fill(0xcd)
    );
    assert.equal(e.length, 108);
    assert.deepEqual(
      e,
      fromHex(
        "03" +
          "11".repeat(32) +
          "22".repeat(32) +
          "00000020" +
          "33".repeat(32) +
          "00000003" +
          "cd".repeat(3)
      )
    );
    assert.deepEqual(e.slice(65, 69), fromHex("00000020")); // vk_len == 32
    assert.equal(e[0], 3); // proof_type
  });
  it("proof over-cap rejected; vk_id must be 32 bytes", () => {
    assert.throws(
      () => buildProofSubmissionGroth16RegisteredExtras(C, CO, VKID, new Uint8Array(1025)),
      RangeError
    );
    assert.throws(
      () => buildProofSubmissionGroth16RegisteredExtras(C, CO, new Uint8Array(31), new Uint8Array(3)),
      RangeError
    );
  });
});

describe("proof submission: code/computation hash guards", () => {
  it("rejects non-32-byte code_hash / computation_hash", () => {
    assert.throws(() => buildProofSubmissionStubExtras(new Uint8Array(31), CO), RangeError);
    assert.throws(
      () => buildProofSubmissionGroth16Extras(C, new Uint8Array(33), new Uint8Array(0), new Uint8Array(0)),
      RangeError
    );
  });
});
