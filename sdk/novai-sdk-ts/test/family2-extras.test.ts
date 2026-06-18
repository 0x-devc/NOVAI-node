/**
 * Golden-vector tests for Family 2 signal extras: ReputationUpdate (7),
 * SignalPurchase (8), StakeSlash (11), CompositionCheck (12).
 *
 * Ground truth (chain execution handler, crates/execution/src/lib.rs):
 *   7:  REPUTATION_UPDATE_EXTRA_LEN=35, total 101; decoder :3032-3041
 *       (event_type@98 u8, points_delta i16 BE @99..101)
 *   8:  SIGNAL_PURCHASE_EXTRA_LEN=41, total 107; decoder :3065-3074
 *       (purchased_type@98 u8, max_price u64 BE @99..107)
 *   11: STAKE_SLASH_EXTRA_LEN=51, total 117; decoder :3163-3175
 *       (slash_amount u128 BE @98..114, rep_event@114 u8, points_delta i16 BE @115..117)
 *   12: COMPOSITION_CHECK_EXTRA_LEN=34, total 100; decoder :3200-3209
 *       (failed_idx@98 u8, failure_reason@99 u8)
 *
 * Hex vectors were produced by the REAL Python reference builders loaded by
 * file path (novai_sdk/signals/{reputation,purchase,stake,composition}.py).
 * This file imports only ../src/signals (no native modules).
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildReputationUpdateExtras,
  buildSignalPurchaseExtras,
  buildStakeSlashExtras,
  buildCompositionCheckExtras,
} from "../src/signals";

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
const id = (b: number) => new Uint8Array(32).fill(b);

describe("reputation extras (type 7): Python golden vectors", () => {
  const T = id(0x11);
  const V: Array<[number, number, string]> = [
    [1, -1, "111111111111111111111111111111111111111111111111111111111111111101ffff"],
    [6, 100, "1111111111111111111111111111111111111111111111111111111111111111060064"],
    [0, -32768, "1111111111111111111111111111111111111111111111111111111111111111008000"],
    [255, 32767, "1111111111111111111111111111111111111111111111111111111111111111ff7fff"],
  ];
  for (const [et, pd, hex] of V) {
    it(`event_type=${et} points_delta=${pd}`, () => {
      const e = buildReputationUpdateExtras(T, et, pd);
      assert.equal(e.length, 35);
      assert.deepEqual(e, fromHex(hex));
    });
  }
  it("points_delta is signed i16 big-endian (-1 -> ff ff)", () => {
    const e = buildReputationUpdateExtras(T, 0, -1);
    assert.equal(e[33], 0xff);
    assert.equal(e[34], 0xff);
  });
});

describe("signal purchase extras (type 8): Python golden vectors", () => {
  const S = id(0x22);
  const V: Array<[number, bigint, string]> = [
    [6, 10n ** 18n, "2222222222222222222222222222222222222222222222222222222222222222060de0b6b3a7640000"],
    [0, 2n ** 64n - 1n, "222222222222222222222222222222222222222222222222222222222222222200ffffffffffffffff"],
    [22, 0n, "2222222222222222222222222222222222222222222222222222222222222222160000000000000000"],
  ];
  for (const [ps, mp, hex] of V) {
    it(`purchased_type=${ps} max_price=${mp}`, () => {
      const e = buildSignalPurchaseExtras(S, ps, mp);
      assert.equal(e.length, 41);
      assert.deepEqual(e, fromHex(hex));
    });
  }
  it("max_price is u64 big-endian (10**18 -> high bytes first)", () => {
    const e = buildSignalPurchaseExtras(S, 0, 10n ** 18n);
    assert.deepEqual(e.slice(33), fromHex("0de0b6b3a7640000"));
  });
});

describe("stake slash extras (type 11): Python golden vectors", () => {
  const T = id(0x44);
  const V: Array<[bigint, number, number, string]> = [
    [10n ** 30n, 6, -5, "44444444444444444444444444444444444444444444444444444444444444440000000c9f2c9cd04674edea4000000006fffb"],
    [2n ** 128n - 1n, 1, -32768, "4444444444444444444444444444444444444444444444444444444444444444ffffffffffffffffffffffffffffffff018000"],
    [0n, 0, 0, "444444444444444444444444444444444444444444444444444444444444444400000000000000000000000000000000000000"],
  ];
  for (const [sa, re, pd, hex] of V) {
    it(`slash_amount=${sa} rep_event=${re} points_delta=${pd}`, () => {
      const e = buildStakeSlashExtras(T, sa, re, pd);
      assert.equal(e.length, 51);
      assert.deepEqual(e, fromHex(hex));
    });
  }
  it("field placement: slash_amount u128 BE @ [32..48], points_delta i16 BE @ [49..51]", () => {
    const e = buildStakeSlashExtras(T, 1n, 0, -5);
    assert.deepEqual(e.slice(32, 48), fromHex("00000000000000000000000000000001"));
    assert.equal(e[48], 0);
    assert.deepEqual(e.slice(49, 51), fromHex("fffb"));
  });
});

describe("composition check extras (type 12): Python golden vectors", () => {
  const T = id(0x33);
  const V: Array<[number, number, string]> = [
    [2, 1, "33333333333333333333333333333333333333333333333333333333333333330201"],
    [0, 0, "33333333333333333333333333333333333333333333333333333333333333330000"],
    [255, 255, "3333333333333333333333333333333333333333333333333333333333333333ffff"],
  ];
  for (const [fi, fr, hex] of V) {
    it(`failed_idx=${fi} reason=${fr}`, () => {
      const e = buildCompositionCheckExtras(T, fi, fr);
      assert.equal(e.length, 34);
      assert.deepEqual(e, fromHex(hex));
    });
  }
});

describe("Family 2 extras: validation guards", () => {
  const good = id(0x01);
  it("rejects short or long entity ids (the 32-byte guard fires)", () => {
    const short = new Uint8Array(31).fill(1);
    const long = new Uint8Array(33).fill(1);
    assert.throws(() => buildReputationUpdateExtras(short, 0, 0), RangeError);
    assert.throws(() => buildReputationUpdateExtras(long, 0, 0), RangeError);
    assert.throws(() => buildSignalPurchaseExtras(short, 0, 0n), RangeError);
    assert.throws(() => buildStakeSlashExtras(short, 0n, 0, 0), RangeError);
    assert.throws(() => buildCompositionCheckExtras(short, 0, 0), RangeError);
  });
  it("rejects u8 fields out of range", () => {
    assert.throws(() => buildReputationUpdateExtras(good, 256, 0), RangeError);
    assert.throws(() => buildSignalPurchaseExtras(good, -1, 0n), RangeError);
    assert.throws(() => buildCompositionCheckExtras(good, 0, 256), RangeError);
  });
  it("rejects i16 points_delta out of range", () => {
    assert.throws(() => buildReputationUpdateExtras(good, 0, 32768), RangeError);
    assert.throws(() => buildReputationUpdateExtras(good, 0, -32769), RangeError);
    assert.throws(() => buildStakeSlashExtras(good, 0n, 0, 32768), RangeError);
  });
  it("rejects u64 max_price and u128 slash_amount out of range", () => {
    assert.throws(() => buildSignalPurchaseExtras(good, 0, 2n ** 64n), RangeError);
    assert.throws(() => buildStakeSlashExtras(good, 2n ** 128n, 0, 0), RangeError);
  });
});
