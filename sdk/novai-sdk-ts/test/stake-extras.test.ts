/**
 * Golden-vector tests for the stake signal extras tails (signal types 9, 10).
 *
 * Ground truth for the layout is the chain execution handler:
 *   crates/execution/src/lib.rs:1208-1222  STAKE_*_EXTRA_LEN = 16, total = 82
 *   crates/execution/src/lib.rs:3107-3115   StakeDeposit:  exact-length check + u128::from_be_bytes
 *   crates/execution/src/lib.rs:3135-3143   StakeWithdraw: exact-length check + u128::from_be_bytes
 *
 * The golden hex vectors below were produced by the REAL Python reference
 * builder sdk/novai-python-sdk/novai_sdk/signals/stake.py
 * (build_stake_deposit_extras), loaded by file path and run with these exact
 * inputs. They also match an independent hand-derivation of a 16-byte
 * big-endian u128. StakeWithdraw is byte-identical to StakeDeposit.
 *
 * This file imports only ../src/signals (pure, no native modules), so the
 * byte-level vectors run even if a native dependency fails to build.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildStakeDepositExtras,
  buildStakeWithdrawExtras,
} from "../src/signals";

const STAKE_EXTRA_LEN = 16;

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

// amount -> Python build_stake_deposit_extras(amount).hex()  (withdraw identical)
const VECTORS: Array<[bigint, string]> = [
  [10n ** 18n, "00000000000000000de0b6b3a7640000"],
  [42n, "0000000000000000000000000000002a"],
  [1n, "00000000000000000000000000000001"],
  [256n, "00000000000000000000000000000100"],
  [2n ** 128n - 1n, "ffffffffffffffffffffffffffffffff"],
];

describe("stake extras: length invariant", () => {
  it("deposit tail is exactly 16 bytes (STAKE_DEPOSIT_EXTRA_LEN)", () => {
    assert.equal(buildStakeDepositExtras(0n).length, STAKE_EXTRA_LEN);
    assert.equal(
      buildStakeDepositExtras(2n ** 128n - 1n).length,
      STAKE_EXTRA_LEN
    );
  });
  it("withdraw tail is exactly 16 bytes (STAKE_WITHDRAW_EXTRA_LEN)", () => {
    assert.equal(buildStakeWithdrawExtras(0n).length, STAKE_EXTRA_LEN);
  });
});

describe("stake extras: Python golden vectors (byte-for-byte)", () => {
  for (const [amount, hex] of VECTORS) {
    it(`deposit(${amount}) == ${hex}`, () => {
      assert.deepEqual(buildStakeDepositExtras(amount), fromHex(hex));
    });
    it(`withdraw(${amount}) == deposit(${amount})`, () => {
      assert.deepEqual(
        buildStakeWithdrawExtras(amount),
        buildStakeDepositExtras(amount)
      );
    });
  }
});

describe("stake extras: big-endian (the most likely silent bug)", () => {
  it("value 1 lands in the LAST byte, not the first", () => {
    const e = buildStakeDepositExtras(1n);
    assert.equal(e[15], 1, "big-endian: LSB at last byte");
    assert.equal(e[0], 0, "big-endian: MSB byte is zero for value 1");
    // A little-endian implementation would put 1 at e[0]; this assertion fails it.
  });
  it("value 256 occupies bytes [14..16] as 01 00", () => {
    const e = buildStakeDepositExtras(256n);
    assert.equal(e[14], 1);
    assert.equal(e[15], 0);
  });
});

describe("stake extras: range validation (mirrors Python guard)", () => {
  it("rejects negative amounts", () => {
    assert.throws(() => buildStakeDepositExtras(-1n), RangeError);
    assert.throws(() => buildStakeWithdrawExtras(-1n), RangeError);
  });
  it("rejects amounts >= 2**128", () => {
    assert.throws(() => buildStakeDepositExtras(2n ** 128n), RangeError);
    assert.throws(() => buildStakeWithdrawExtras(2n ** 128n), RangeError);
  });
});
