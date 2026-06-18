/**
 * Golden-vector tests for Family 4 extras + the channel-state signing message:
 * SlaAccept (18), ChannelAccept (19), ChannelClose (20), ChannelFinalize (21),
 * and channelStateSigningBytes.
 *
 * Ground truth (chain execution handler, crates/execution/src/lib.rs):
 *   18/19/21: 64-byte tails [id:32][id:32], total 130; decoders :3606-3668, :3732-3743
 *   20 ChannelClose: 233-byte tail, total 299; decoder :3670-3704
 *       channel_id[0..32] party_a[32..64] nonce u64 BE[64..72]
 *       balance_a u128 BE[72..88] balance_b u128 BE[88..104] is_final[104]
 *       sig_a[105..169] sig_b[169..233]
 *   channelStateSigningBytes: 167-byte message, crates/crypto/src/lib.rs:33,52-74
 *       "NOVAI_CHANNEL_STATE_V1"(22) chain_id u64 BE[22..30] channel_id[30..62]
 *       party_a[62..94] party_b[94..126] nonce u64 BE[126..134]
 *       balance_a u128 BE[134..150] balance_b u128 BE[150..166] is_final[166]
 *
 * Tail + signing-bytes vectors were produced by the real Python builders
 * (signals/{sla,channels}.py, crypto.py channel_state_signing_bytes; blake3/nacl
 * stubbed for import since those pure functions do not use them). Imports only
 * ../src/signals (no native modules).
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildSlaAcceptExtras,
  buildChannelAcceptExtras,
  buildChannelCloseExtras,
  buildChannelFinalizeExtras,
  channelStateSigningBytes,
} from "../src/signals";

function fromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
const id = (b: number) => new Uint8Array(32).fill(b);
const sig = (b: number) => new Uint8Array(64).fill(b);

describe("sla/channel 64-byte extras (types 18, 19, 21): Python golden vectors", () => {
  it("sla accept [sla_object_id][buyer]", () => {
    const e = buildSlaAcceptExtras(id(0x77), id(0x88));
    assert.equal(e.length, 64);
    assert.deepEqual(e, fromHex("77".repeat(32) + "88".repeat(32)));
  });
  it("channel accept [channel_object_id][party_a]", () => {
    const e = buildChannelAcceptExtras(id(0xaa), id(0xbb));
    assert.equal(e.length, 64);
    assert.deepEqual(e, fromHex("aa".repeat(32) + "bb".repeat(32)));
  });
  it("channel finalize [channel_object_id][party_a]", () => {
    const e = buildChannelFinalizeExtras(id(0xaa), id(0xbb));
    assert.equal(e.length, 64);
    assert.deepEqual(e, fromHex("aa".repeat(32) + "bb".repeat(32)));
  });
});

describe("channel close extras (type 20): 233-byte tail", () => {
  const PY =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb000000000000002a000000000000000000000000000003e8000000000000000000000000000001f4000101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010102020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202";
  it("matches Python golden vector byte-for-byte", () => {
    const e = buildChannelCloseExtras(
      id(0xaa),
      id(0xbb),
      42n,
      1000n,
      500n,
      false,
      sig(0x01),
      sig(0x02)
    );
    assert.equal(e.length, 233);
    assert.deepEqual(e, fromHex(PY));
  });
  it("field placement: nonce u64 BE, two u128 BE balances, is_final, two sigs", () => {
    const e = buildChannelCloseExtras(
      id(0xaa),
      id(0xbb),
      42n,
      1000n,
      500n,
      true,
      sig(0x01),
      sig(0x02)
    );
    assert.deepEqual(e.slice(64, 72), fromHex("000000000000002a")); // nonce=42 BE
    assert.deepEqual(
      e.slice(72, 88),
      fromHex("000000000000000000000000000003e8")
    ); // balance_a=1000 BE
    assert.deepEqual(
      e.slice(88, 104),
      fromHex("000000000000000000000000000001f4")
    ); // balance_b=500 BE
    assert.equal(e[104], 1); // is_final true
    assert.deepEqual(e.slice(105, 169), sig(0x01)); // sig_a
    assert.deepEqual(e.slice(169, 233), sig(0x02)); // sig_b
  });
  it("is_final false writes 0x00", () => {
    const e = buildChannelCloseExtras(
      id(0xaa),
      id(0xbb),
      0n,
      0n,
      0n,
      false,
      sig(0),
      sig(0)
    );
    assert.equal(e[104], 0);
  });
});

describe("channelStateSigningBytes (167-byte consensus-critical message)", () => {
  const PY =
    "4e4f5641495f4348414e4e454c5f53544154455f56310000000000000001aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc000000000000002a000000000000000000000000000003e8000000000000000000000000000001f400";
  it("matches Python golden vector byte-for-byte (chain_id=1)", () => {
    const m = channelStateSigningBytes(
      id(0xaa),
      id(0xbb),
      id(0xcc),
      42n,
      1000n,
      500n,
      false
    );
    assert.equal(m.length, 167);
    assert.deepEqual(m, fromHex(PY));
  });
  it("starts with the exact domain tag NOVAI_CHANNEL_STATE_V1 (distinct from the SLA tag)", () => {
    const m = channelStateSigningBytes(id(0xaa), id(0xbb), id(0xcc), 0n, 0n, 0n, false);
    assert.deepEqual(
      m.slice(0, 22),
      fromHex("4e4f5641495f4348414e4e454c5f53544154455f5631")
    );
  });
  it("chain_id is a u64 BE parameter at [22..30] (default 1, overridable)", () => {
    const def = channelStateSigningBytes(id(0xaa), id(0xbb), id(0xcc), 0n, 0n, 0n, false);
    assert.deepEqual(def.slice(22, 30), fromHex("0000000000000001"));
    const seven = channelStateSigningBytes(id(0xaa), id(0xbb), id(0xcc), 0n, 0n, 0n, false, 7n);
    assert.deepEqual(seven.slice(22, 30), fromHex("0000000000000007"));
  });
});

describe("Family 4 extras: validation guards", () => {
  const g = id(0x01);
  it("rejects non-32-byte ids and non-64-byte sigs", () => {
    assert.throws(() => buildSlaAcceptExtras(new Uint8Array(31), g), RangeError);
    assert.throws(
      () => buildChannelCloseExtras(g, g, 0n, 0n, 0n, false, new Uint8Array(63), sig(0)),
      RangeError
    );
    assert.throws(
      () => buildChannelCloseExtras(g, g, 0n, 0n, 0n, false, sig(0), new Uint8Array(65)),
      RangeError
    );
  });
  it("rejects u64 nonce and u128 balances out of range", () => {
    assert.throws(
      () => buildChannelCloseExtras(g, g, 2n ** 64n, 0n, 0n, false, sig(0), sig(0)),
      RangeError
    );
    assert.throws(
      () => buildChannelCloseExtras(g, g, 0n, 2n ** 128n, 0n, false, sig(0), sig(0)),
      RangeError
    );
    assert.throws(
      () => channelStateSigningBytes(g, g, g, 0n, 0n, 0n, false, 2n ** 64n),
      RangeError
    );
  });
});
