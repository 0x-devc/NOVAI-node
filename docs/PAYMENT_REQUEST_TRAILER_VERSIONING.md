# PaymentRequest trailer dispatch: design debt

**Status:** Noted, not blocking. Surfaced during the Phase 0 Python SDK
research (Week 37).

## Current state

The `PaymentRequest` signal (signal type 16) has accreted optional trailers
across Weeks 28, 33, and 36. The current decoder dispatches on the byte at
offset 178 of the payload (immediately after the legacy 178-byte tail):

| Byte at offset 178   | Trailer kind                                  | Layout                                                 |
| -------------------- | --------------------------------------------- | ------------------------------------------------------ |
| absent (payload=178) | Legacy single-recipient payment (Week 28)     | none                                                   |
| 2..=8                | Multi-party splits (Week 33)                  | count(1) + N * (recipient(32) + bp(2 BE))              |
| 0xC1                 | Conditional execution marker (Week 36)        | marker(1) + kind(1) + operand(var) + optional splits   |

Any other byte value is a decode error. The marker byte 0xC1 was chosen
specifically because it sits outside the legal splits-count range [2, 8],
which is what allows the current decoder to branch unambiguously.

## Why this is debt

The dispatch namespace is implicit: the splits-count range [2, 8] occupies
the low end of the byte space, and the condition marker 0xC1 occupies a
single isolated value. There is no version byte, no length prefix, and no
type tag. Adding a third trailer would require:

1. Picking a new byte value that does not collide with either
   `[2, 8]` (splits-count) or `0xC1` (condition marker).
2. Updating every decoder branch in lockstep.
3. Hoping no future trailer needs a fourth, fifth, or Nth marker, because
   each one further shrinks the free byte space.

This works for two trailers. It works fairly clumsily for three. By the
time we have five or six features wanting their own trailer, the decoder
becomes a manual byte-allocation table that is impossible to extend
safely without a hard fork.

## Suggested future fix

When a third trailer type is needed, introduce a proper trailer
versioning scheme. Two reasonable shapes:

* **Tagged TLV trailer**: after the 178-byte legacy tail, every trailer
  starts with a single-byte type tag (e.g. 0x01 = splits, 0x02 = condition,
  0x03 = future-X) and a length prefix. Multiple trailers can compose.
  Decoders read tags until end-of-payload.

* **Versioned extras envelope**: wrap all optional trailers in a single
  envelope with its own version byte. Bump the version when adding a new
  trailer type. Old clients reject unknown versions cleanly.

Both approaches resolve the byte-allocation problem and let new trailers
ship without coordinating against the existing namespace.

## When to act

Not now. The Week 33 and Week 36 trailers are frozen and decode correctly.
The first new feature that proposes its own PaymentRequest trailer is the
forcing function: at that point, instead of adding a third magic byte,
take the opportunity to introduce a proper TLV or versioned-envelope
scheme and migrate the existing trailers into it (with full backwards
compatibility for the byte-for-byte legacy decodings).

Until then, this file exists to make sure the design constraint is
visible the next time someone reaches for a fourth magic byte.

## See also

* `crates/execution/src/lib.rs:1295-1480` for the relevant constants
  (MAX_PAYMENT_SPLITS, MIN_PAYMENT_SPLITS_WHEN_PRESENT,
  PAYMENT_CONDITION_MARKER, etc.).
* `docs/DEVLOG.md` Weeks 28, 33, and 36 for the per-feature designs.
