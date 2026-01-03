# Architecture Decisions (Locked)

1. Determinism over performance
2. No floating point in consensus or state
3. HotStuff-like BFT, original implementation
4. Explicit state machine boundaries
5. Canonical encoding for all wire/state formats
6. License safety enforced by tooling
7. Minimal dependencies
8. Signals-only AI integration
9. Cryptography isolated in its own crate
10. Networking isolated from consensus
11. Test vectors required for formats
12. Clean-room provenance preserved
### Test vectors (Week 2)

We commit golden binary test vectors for consensus/network formats to detect encoding drift.

- Location: `crates/codec/tests/vectors/*.bin`
- Test: `crates/codec/tests/golden_vectors.rs`
- Run: `cargo test -p novai-codec`
- Regenerate (intentional): `UPDATE_VECTORS=1 cargo test -p novai-codec`
