/**
 * Golden-vector tests for Family 3 signal extras: SubscriptionCreate (14),
 * SubscriptionCancel (15).
 *
 * Ground truth (chain execution handler, crates/execution/src/lib.rs):
 *   14: SUBSCRIPTION_CREATE_EXTRA_LEN=49, total 115; decoder :3348-3377
 *       (covered_signal_type@98 u8, rate_per_block u64 BE @99..107,
 *        duration_blocks u64 BE @107..115)
 *   15: SUBSCRIPTION_CANCEL_EXTRA_LEN=32, total 98; decoder :3401-3409
 *       (subscription_id@66..98)
 *
 * Python-parity vectors were produced by the real builders
 * (novai_sdk/signals/subscription.py). The duration=1 case is HAND-DERIVED
 * (plain u64 BE), NOT Python-parity: the Python builder refuses
 * duration < MIN_SUBSCRIPTION_DURATION (100), a chain-owned semantic floor the
 * TS builder intentionally does not enforce (encode bytes, chain is authority).
 * Imports only ../src/signals.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildSubscriptionCreateExtras,
  buildSubscriptionCancelExtras,
} from "../src/signals";

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
const id = (b: number) => new Uint8Array(32).fill(b);

describe("subscription create extras (type 14): Python golden vectors", () => {
  const P = id(0x55);
  // [covered, rate, duration, hex] from Python build_subscription_create_extras
  const V: Array<[number, bigint, bigint, string]> = [
    [6, 10n ** 18n, 1000n, "5555555555555555555555555555555555555555555555555555555555555555060de0b6b3a764000000000000000003e8"],
    [0, 2n ** 64n - 1n, 100n, "555555555555555555555555555555555555555555555555555555555555555500ffffffffffffffff0000000000000064"],
  ];
  for (const [cov, rate, dur, hex] of V) {
    it(`covered=${cov} rate=${rate} duration=${dur}`, () => {
      const e = buildSubscriptionCreateExtras(P, cov, rate, dur);
      assert.equal(e.length, 49);
      assert.deepEqual(e, fromHex(hex));
    });
  }

  it("HAND-DERIVED duration=1 (Python refuses < 100; TS does not enforce the floor)", () => {
    // producer(32) + covered(0x06) + rate=1 (u64 BE) + duration=1 (u64 BE)
    const expected =
      "55".repeat(32) + "06" + "00".repeat(7) + "01" + "00".repeat(7) + "01";
    const e = buildSubscriptionCreateExtras(P, 6, 1n, 1n);
    assert.equal(e.length, 49);
    assert.deepEqual(e, fromHex(expected));
  });

  it("rate_per_block and duration_blocks are u64 big-endian", () => {
    const e = buildSubscriptionCreateExtras(P, 0, 1n, 1000n);
    assert.deepEqual(e.slice(33, 41), fromHex("0000000000000001")); // rate=1 BE
    assert.deepEqual(e.slice(41, 49), fromHex("00000000000003e8")); // duration=1000 BE
  });
});

describe("subscription cancel extras (type 15): Python golden vector", () => {
  it("subscription_id passthrough (32 bytes)", () => {
    const e = buildSubscriptionCancelExtras(id(0x66));
    assert.equal(e.length, 32);
    assert.deepEqual(
      e,
      fromHex("6666666666666666666666666666666666666666666666666666666666666666")
    );
  });
  it("returns a copy (caller mutation does not alias the tail)", () => {
    const src = id(0x66);
    const e = buildSubscriptionCancelExtras(src);
    src[0] = 0xff;
    assert.equal(e[0], 0x66);
  });
});

describe("Family 3 extras: validation guards", () => {
  const good = id(0x01);
  it("rejects non-32-byte ids", () => {
    const short = new Uint8Array(31).fill(1);
    assert.throws(() => buildSubscriptionCreateExtras(short, 0, 0n, 0n), RangeError);
    assert.throws(() => buildSubscriptionCancelExtras(short), RangeError);
    assert.throws(() => buildSubscriptionCancelExtras(new Uint8Array(33)), RangeError);
  });
  it("rejects u8 covered_signal_type out of range", () => {
    assert.throws(() => buildSubscriptionCreateExtras(good, 256, 0n, 0n), RangeError);
  });
  it("rejects u64 rate/duration out of range", () => {
    assert.throws(() => buildSubscriptionCreateExtras(good, 0, 2n ** 64n, 0n), RangeError);
    assert.throws(() => buildSubscriptionCreateExtras(good, 0, 0n, 2n ** 64n), RangeError);
    assert.throws(() => buildSubscriptionCreateExtras(good, 0, -1n, 0n), RangeError);
  });
});
