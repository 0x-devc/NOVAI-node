/**
 * Tests for the signalCommitment() extras passthrough: the 66-byte envelope
 * followed by a type-specific tail, total length matching the chain execution
 * handler's SIGNAL_COMMITMENT_PAYLOAD_V1_*_LEN constants in
 * crates/execution/src/lib.rs.
 *
 * Importing ../src/tx and ../src/keys transitively loads the native blake3 and
 * tweetnacl modules, so these tests require a successful `npm install`.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { signalCommitment } from "../src/tx";
import { generateKeypair } from "../src/keys";
import { SignalType, PaymentAttestationStatus } from "../src/types";
import {
  buildStakeDepositExtras,
  buildStakeWithdrawExtras,
  buildReputationUpdateExtras,
  buildSignalPurchaseExtras,
  buildStakeSlashExtras,
  buildCompositionCheckExtras,
  buildSubscriptionCreateExtras,
  buildSubscriptionCancelExtras,
  buildSlaAcceptExtras,
  buildChannelAcceptExtras,
  buildChannelCloseExtras,
  buildChannelFinalizeExtras,
  buildProofSubmissionStubExtras,
  buildProofSubmissionGroth16Extras,
  buildProofSubmissionGroth16RegisteredExtras,
  buildPaymentRequestExtras,
  buildServiceAttestationExtras,
  buildOracleAnchorExtras,
  paymentConditionAnchorExists,
} from "../src/signals";

const BASE_LEN = 66;
const STAKE_TOTAL_LEN = 82;

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

describe("signalCommitment extras passthrough (Family 1: stake)", () => {
  const kp = generateKeypair();
  const signalHash = new Uint8Array(32).fill(0x11);
  const issuer = new Uint8Array(32).fill(0x22);

  it("no extras -> legacy 66-byte envelope (types 0-6 unchanged)", () => {
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.Anomaly,
      issuer
    );
    assert.equal(tx.payload.length, BASE_LEN);
    assert.equal(tx.payload[0], 2);
    assert.equal(tx.payload[33], SignalType.Anomaly);
  });

  it("stake deposit -> 82-byte payload, tail at [66..82], type byte 9", () => {
    assert.equal(SignalType.StakeDeposit, 9);
    const tail = buildStakeDepositExtras(10n ** 18n);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.StakeDeposit,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, STAKE_TOTAL_LEN);
    assert.equal(tx.payload[0], 2);
    assert.equal(tx.payload[33], 9);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
    assert.deepEqual(
      tx.payload.slice(BASE_LEN),
      fromHex("00000000000000000de0b6b3a7640000")
    );
    assert.deepEqual(tx.payload.slice(1, 33), signalHash);
    assert.deepEqual(tx.payload.slice(34, 66), issuer);
  });

  it("stake withdraw -> type byte 10, 82-byte payload", () => {
    assert.equal(SignalType.StakeWithdraw, 10);
    const tail = buildStakeWithdrawExtras(42n);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.StakeWithdraw,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, STAKE_TOTAL_LEN);
    assert.equal(tx.payload[33], 10);
  });
});

describe("signalCommitment extras passthrough (Family 2: rep/purchase/slash/composition)", () => {
  const kp = generateKeypair();
  const signalHash = new Uint8Array(32).fill(0x11);
  const issuer = new Uint8Array(32).fill(0x22);
  const target = new Uint8Array(32).fill(0x33);

  it("reputation update -> 101-byte payload, type byte 7, tail at [66..]", () => {
    assert.equal(SignalType.ReputationUpdate, 7);
    const tail = buildReputationUpdateExtras(target, 1, -1);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.ReputationUpdate,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, 101);
    assert.equal(tx.payload[33], 7);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("signal purchase -> 107-byte payload, type byte 8", () => {
    assert.equal(SignalType.SignalPurchase, 8);
    const tail = buildSignalPurchaseExtras(target, 6, 10n ** 18n);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.SignalPurchase,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, 107);
    assert.equal(tx.payload[33], 8);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("stake slash -> 117-byte payload, type byte 11", () => {
    assert.equal(SignalType.StakeSlash, 11);
    const tail = buildStakeSlashExtras(target, 10n ** 18n, 6, -5);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.StakeSlash,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, 117);
    assert.equal(tx.payload[33], 11);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("composition check -> 100-byte payload, type byte 12", () => {
    assert.equal(SignalType.CompositionCheck, 12);
    const tail = buildCompositionCheckExtras(target, 2, 1);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.CompositionCheck,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, 100);
    assert.equal(tx.payload[33], 12);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });
});

describe("signalCommitment extras passthrough (Family 3: subscription)", () => {
  const kp = generateKeypair();
  const signalHash = new Uint8Array(32).fill(0x11);
  const issuer = new Uint8Array(32).fill(0x22);
  const target = new Uint8Array(32).fill(0x33);

  it("subscription create -> 115-byte payload, type byte 14", () => {
    assert.equal(SignalType.SubscriptionCreate, 14);
    const tail = buildSubscriptionCreateExtras(target, 6, 10n ** 18n, 1000n);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.SubscriptionCreate,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, 115);
    assert.equal(tx.payload[33], 14);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("subscription cancel -> 98-byte payload, type byte 15", () => {
    assert.equal(SignalType.SubscriptionCancel, 15);
    const tail = buildSubscriptionCancelExtras(target);
    const tx = signalCommitment(
      kp,
      0n,
      1n,
      signalHash,
      SignalType.SubscriptionCancel,
      issuer,
      tail
    );
    assert.equal(tx.payload.length, 98);
    assert.equal(tx.payload[33], 15);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });
});

describe("signalCommitment extras passthrough (Family 4: sla + channels)", () => {
  const kp = generateKeypair();
  const signalHash = new Uint8Array(32).fill(0x11);
  const issuer = new Uint8Array(32).fill(0x22);
  const a = new Uint8Array(32).fill(0x33);
  const b = new Uint8Array(32).fill(0x44);
  const s = new Uint8Array(64).fill(0x55);

  it("sla accept -> 130-byte payload, type byte 18", () => {
    assert.equal(SignalType.SlaAccept, 18);
    const tail = buildSlaAcceptExtras(a, b);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.SlaAccept, issuer, tail);
    assert.equal(tx.payload.length, 130);
    assert.equal(tx.payload[33], 18);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("channel accept -> 130-byte payload, type byte 19", () => {
    assert.equal(SignalType.ChannelAccept, 19);
    const tail = buildChannelAcceptExtras(a, b);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ChannelAccept, issuer, tail);
    assert.equal(tx.payload.length, 130);
    assert.equal(tx.payload[33], 19);
  });

  it("channel close -> 299-byte payload, type byte 20", () => {
    assert.equal(SignalType.ChannelClose, 20);
    const tail = buildChannelCloseExtras(a, b, 42n, 1000n, 500n, false, s, s);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ChannelClose, issuer, tail);
    assert.equal(tx.payload.length, 299);
    assert.equal(tx.payload[33], 20);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("channel finalize -> 130-byte payload, type byte 21", () => {
    assert.equal(SignalType.ChannelFinalize, 21);
    const tail = buildChannelFinalizeExtras(a, b);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ChannelFinalize, issuer, tail);
    assert.equal(tx.payload.length, 130);
    assert.equal(tx.payload[33], 21);
  });
});

describe("signalCommitment extras passthrough (Family 5: proof submission)", () => {
  const kp = generateKeypair();
  const signalHash = new Uint8Array(32).fill(0x11);
  const issuer = new Uint8Array(32).fill(0x22);
  const c = new Uint8Array(32).fill(0x33);
  const co = new Uint8Array(32).fill(0x44);

  it("proof stub -> 131-byte payload, type 13, proof_type byte 0 at [66]", () => {
    assert.equal(SignalType.ProofSubmission, 13);
    const tail = buildProofSubmissionStubExtras(c, co);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ProofSubmission, issuer, tail);
    assert.equal(tx.payload.length, 131);
    assert.equal(tx.payload[33], 13);
    assert.equal(tx.payload[66], 0);
  });

  it("proof groth16 inline -> 66 + tail, proof_type byte 1", () => {
    const tail = buildProofSubmissionGroth16Extras(
      c,
      co,
      new Uint8Array(5).fill(0xab),
      new Uint8Array(3).fill(0xcd)
    );
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ProofSubmission, issuer, tail);
    assert.equal(tx.payload.length, 66 + tail.length);
    assert.equal(tx.payload[66], 1);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("proof registered -> 174-byte payload, proof_type byte 3", () => {
    const tail = buildProofSubmissionGroth16RegisteredExtras(
      c,
      co,
      new Uint8Array(32).fill(0x55),
      new Uint8Array(3).fill(0xcd)
    );
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ProofSubmission, issuer, tail);
    assert.equal(tx.payload.length, 174);
    assert.equal(tx.payload[66], 3);
  });
});

describe("signalCommitment extras passthrough (Family 6: payments + oracle)", () => {
  const kp = generateKeypair();
  const signalHash = new Uint8Array(32).fill(0x11);
  const issuer = new Uint8Array(32).fill(0x22);
  const a = new Uint8Array(32).fill(0x33);
  const b = new Uint8Array(32).fill(0x44);

  it("payment request base -> 178-byte payload, type 16", () => {
    assert.equal(SignalType.PaymentRequest, 16);
    const tail = buildPaymentRequestExtras(a, 1000n, b, a, 5000n);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.PaymentRequest, issuer, tail);
    assert.equal(tx.payload.length, 178);
    assert.equal(tx.payload[33], 16);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });

  it("payment request with condition -> 0xC1 marker at full offset 178", () => {
    const tail = buildPaymentRequestExtras(a, 1000n, b, a, 5000n, {
      condition: paymentConditionAnchorExists(a),
    });
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.PaymentRequest, issuer, tail);
    assert.equal(tx.payload.length, 66 + tail.length);
    assert.equal(tx.payload[178], 0xc1);
  });

  it("service attestation -> 131-byte payload, type 17", () => {
    assert.equal(SignalType.ServiceAttestation, 17);
    const tail = buildServiceAttestationExtras(a, b, PaymentAttestationStatus.Delivered);
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.ServiceAttestation, issuer, tail);
    assert.equal(tx.payload.length, 131);
    assert.equal(tx.payload[33], 17);
  });

  it("oracle anchor -> 66 + tail, type 22", () => {
    assert.equal(SignalType.OracleAnchor, 22);
    const tail = buildOracleAnchorExtras(a, 1234n, b, 9999n, new Uint8Array([0xaa]));
    const tx = signalCommitment(kp, 0n, 1n, signalHash, SignalType.OracleAnchor, issuer, tail);
    assert.equal(tx.payload.length, 66 + tail.length);
    assert.equal(tx.payload[33], 22);
    assert.deepEqual(tx.payload.slice(BASE_LEN), tail);
  });
});
