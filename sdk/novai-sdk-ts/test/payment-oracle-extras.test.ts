/**
 * Golden-vector tests for Family 6 signal extras: PaymentRequest (16),
 * ServiceAttestation (17), OracleAnchor (22).
 *
 * Ground truth (chain execution handler, crates/execution/src/lib.rs):
 *   PaymentRequest base 178 (tail 112): payee[66..98], amount u64 BE[98..106],
 *     service_descriptor_hash[106..138], request_hash[138..170],
 *     max_block_height u64 BE[170..178]; dispatch byte at [178] (tail 112):
 *     [2,8] = splits count, 0xC1 = condition, else rejected. Decoder :3428-3568,
 *     decode_payment_splits :2142, decode_payment_condition :2166.
 *   ServiceAttestation 131 (tail 65): payment_signal_hash, payee, status{0,1}. :3569-3605
 *   OracleAnchor 148..=179 (tail 82..=113): data_hash, external_timestamp u64 BE,
 *     source_hash, expiry_height u64 BE, data_tag_len:1, data_tag:1..=32. :3764-3792
 *
 * Vectors reproduce the real Python builders (signals/{payments,oracle}.py),
 * assembled here from documented field bytes. Imports only ../src/signals.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildPaymentRequestExtras,
  buildServiceAttestationExtras,
  buildOracleAnchorExtras,
  validatePaymentSplits,
  paymentConditionAnchorExists,
  paymentConditionAnchorDataHashEquals,
  paymentConditionAnchorTagEquals,
  paymentConditionAnchorNotExpired,
  PaymentSplit,
} from "../src/signals";
import { PaymentAttestationStatus } from "../src/types";

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
const id = (b: number) => new Uint8Array(32).fill(b);
const rep = (hexByte: string, n: number) => hexByte.repeat(n);

const PAYEE = id(0x11);
const SD = id(0x22);
const REQ = id(0x33);
const ANCHOR = id(0x55);
const EDH = id(0x66);

// Documented PaymentRequest base (amount=1000=0x3e8, max_block=5000=0x1388).
const BASE =
  rep("11", 32) + "00000000000003e8" + rep("22", 32) + rep("33", 32) + "0000000000001388";

describe("payment request (type 16): Python golden vectors", () => {
  it("base (legacy, 112 bytes); amount/max_block u64 BE", () => {
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n);
    assert.equal(e.length, 112);
    assert.deepEqual(e, fromHex(BASE));
    assert.deepEqual(e.slice(32, 40), fromHex("00000000000003e8"));
    assert.deepEqual(e.slice(104, 112), fromHex("0000000000001388"));
  });

  it("with splits (2 entries): dispatch 0x02, basis_points u16 BE (1770 / 0fa0)", () => {
    const s2: PaymentSplit[] = [
      { recipientEntityId: PAYEE, basisPoints: 6000 },
      { recipientEntityId: id(0x44), basisPoints: 4000 },
    ];
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, { splits: s2 });
    assert.equal(e.length, 181);
    assert.equal(e[112], 2);
    assert.deepEqual(
      e,
      fromHex(BASE + "02" + rep("11", 32) + "1770" + rep("44", 32) + "0fa0")
    );
  });

  it("condition kind 1 anchor_exists: 0xC1 marker + kind 1 + anchor", () => {
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
      condition: paymentConditionAnchorExists(ANCHOR),
    });
    assert.equal(e.length, 146);
    assert.equal(e[112], 0xc1);
    assert.equal(e[113], 1);
    assert.deepEqual(e, fromHex(BASE + "c101" + rep("55", 32)));
  });

  it("condition kind 2 data_hash_equals (66-byte trailer)", () => {
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
      condition: paymentConditionAnchorDataHashEquals(ANCHOR, EDH),
    });
    assert.equal(e.length, 178);
    assert.deepEqual(e, fromHex(BASE + "c102" + rep("55", 32) + rep("66", 32)));
  });

  it("condition kind 3 tag_equals (tag='price', tag_len single byte)", () => {
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
      condition: paymentConditionAnchorTagEquals(ANCHOR, fromHex("7072696365")),
    });
    assert.equal(e.length, 152);
    assert.deepEqual(e, fromHex(BASE + "c103" + rep("55", 32) + "05" + "7072696365"));
  });

  it("condition kind 4 not_expired", () => {
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
      condition: paymentConditionAnchorNotExpired(ANCHOR),
    });
    assert.equal(e.length, 146);
    assert.deepEqual(e, fromHex(BASE + "c104" + rep("55", 32)));
  });

  it("condition then splits (combined, CLI order)", () => {
    const s2: PaymentSplit[] = [
      { recipientEntityId: PAYEE, basisPoints: 6000 },
      { recipientEntityId: id(0x44), basisPoints: 4000 },
    ];
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
      condition: paymentConditionAnchorExists(ANCHOR),
      splits: s2,
    });
    assert.equal(e.length, 215);
    assert.deepEqual(
      e,
      fromHex(
        BASE + "c101" + rep("55", 32) + "02" + rep("11", 32) + "1770" + rep("44", 32) + "0fa0"
      )
    );
  });

  it("8 splits: structural (length 385, dispatch byte = 8)", () => {
    const s8: PaymentSplit[] = [{ recipientEntityId: PAYEE, basisPoints: 3000 }];
    for (let i = 1; i < 8; i++) {
      s8.push({ recipientEntityId: id(0x40 + i), basisPoints: 1000 });
    }
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, { splits: s8 });
    assert.equal(e.length, 385); // 112 + 1 + 8*34
    assert.equal(e[112], 8);
  });
});

describe("payment request: structural guards (builder enforces wire-range only)", () => {
  it("rejects split count outside [2,8]", () => {
    assert.throws(
      () =>
        buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
          splits: [{ recipientEntityId: PAYEE, basisPoints: 10000 }],
        }),
      RangeError
    );
    const nine: PaymentSplit[] = Array.from({ length: 9 }, () => ({
      recipientEntityId: PAYEE,
      basisPoints: 1,
    }));
    assert.throws(
      () => buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, { splits: nine }),
      RangeError
    );
  });
  it("rejects bad u64 amount and non-32 ids", () => {
    assert.throws(() => buildPaymentRequestExtras(PAYEE, 2n ** 64n, SD, REQ, 5000n), RangeError);
    assert.throws(
      () => buildPaymentRequestExtras(new Uint8Array(31), 1000n, SD, REQ, 5000n),
      RangeError
    );
  });
  it("rejects condition tag outside 1..=32", () => {
    assert.throws(
      () =>
        buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
          condition: paymentConditionAnchorTagEquals(ANCHOR, new Uint8Array(33)),
        }),
      RangeError
    );
    assert.throws(
      () =>
        buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, {
          condition: paymentConditionAnchorTagEquals(ANCHOR, new Uint8Array(0)),
        }),
      RangeError
    );
  });
});

describe("validatePaymentSplits (opt-in semantic checks)", () => {
  const ok: PaymentSplit[] = [
    { recipientEntityId: PAYEE, basisPoints: 6000 },
    { recipientEntityId: id(0x44), basisPoints: 4000 },
  ];
  it("accepts a valid set (sum 10000, splits[0]==payee, distinct)", () => {
    validatePaymentSplits(ok, PAYEE);
  });
  it("rejects sum != 10000", () => {
    assert.throws(
      () =>
        validatePaymentSplits(
          [
            { recipientEntityId: PAYEE, basisPoints: 6000 },
            { recipientEntityId: id(0x44), basisPoints: 3999 },
          ],
          PAYEE
        ),
      RangeError
    );
  });
  it("rejects splits[0] != payee", () => {
    assert.throws(
      () =>
        validatePaymentSplits(
          [
            { recipientEntityId: id(0x44), basisPoints: 6000 },
            { recipientEntityId: PAYEE, basisPoints: 4000 },
          ],
          PAYEE
        ),
      RangeError
    );
  });
  it("rejects duplicate recipients", () => {
    assert.throws(
      () =>
        validatePaymentSplits(
          [
            { recipientEntityId: PAYEE, basisPoints: 5000 },
            { recipientEntityId: PAYEE, basisPoints: 5000 },
          ],
          PAYEE
        ),
      RangeError
    );
  });
  it("is NOT auto-called by the builder (wire-valid but sum!=10000 still builds)", () => {
    const bad: PaymentSplit[] = [
      { recipientEntityId: PAYEE, basisPoints: 1 },
      { recipientEntityId: id(0x44), basisPoints: 1 },
    ];
    const e = buildPaymentRequestExtras(PAYEE, 1000n, SD, REQ, 5000n, { splits: bad });
    assert.equal(e.length, 181); // builder accepts; the chain is the authority
  });
});

describe("service attestation (type 17): Python golden vectors", () => {
  it("delivered (status 0)", () => {
    const e = buildServiceAttestationExtras(id(0x77), id(0x88), PaymentAttestationStatus.Delivered);
    assert.equal(e.length, 65);
    assert.deepEqual(e, fromHex(rep("77", 32) + rep("88", 32) + "00"));
  });
  it("failed (status 1)", () => {
    const e = buildServiceAttestationExtras(id(0x77), id(0x88), PaymentAttestationStatus.Failed);
    assert.deepEqual(e, fromHex(rep("77", 32) + rep("88", 32) + "01"));
  });
  it("rejects status outside {0,1}", () => {
    assert.throws(
      () => buildServiceAttestationExtras(id(0x77), id(0x88), 2 as PaymentAttestationStatus),
      RangeError
    );
  });
});

describe("oracle anchor (type 22): Python golden vectors", () => {
  // ts=1234=0x4d2, expiry=9999=0x270f
  it("min tag (1 byte); data_tag_len is a single byte at [80]", () => {
    const e = buildOracleAnchorExtras(id(0x11), 1234n, id(0x22), 9999n, fromHex("aa"));
    assert.equal(e.length, 82);
    assert.deepEqual(
      e,
      fromHex(
        rep("11", 32) + "00000000000004d2" + rep("22", 32) + "000000000000270f" + "01" + "aa"
      )
    );
    assert.equal(e[80], 1);
  });
  it("max tag (32 bytes)", () => {
    const e = buildOracleAnchorExtras(id(0x11), 1234n, id(0x22), 9999n, id(0xbb));
    assert.equal(e.length, 113);
    assert.deepEqual(
      e,
      fromHex(
        rep("11", 32) + "00000000000004d2" + rep("22", 32) + "000000000000270f" + "20" + rep("bb", 32)
      )
    );
    assert.equal(e[80], 32);
  });
  it("null source -> 32 zero bytes at [40..72]", () => {
    const e = buildOracleAnchorExtras(id(0x11), 1234n, null, 9999n, fromHex("78"));
    assert.deepEqual(
      e,
      fromHex(
        rep("11", 32) + "00000000000004d2" + rep("00", 32) + "000000000000270f" + "01" + "78"
      )
    );
    assert.deepEqual(e.slice(40, 72), new Uint8Array(32));
  });
  it("rejects tag outside 1..=32 and bad u64 timestamp", () => {
    assert.throws(() => buildOracleAnchorExtras(id(0x11), 1234n, id(0x22), 9999n, new Uint8Array(0)), RangeError);
    assert.throws(() => buildOracleAnchorExtras(id(0x11), 1234n, id(0x22), 9999n, new Uint8Array(33)), RangeError);
    assert.throws(() => buildOracleAnchorExtras(id(0x11), 2n ** 64n, id(0x22), 9999n, fromHex("aa")), RangeError);
  });
});
