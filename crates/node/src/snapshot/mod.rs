//! Snapshot verification core: the A0 audit pipeline (checks A1..A8), the key
//! classification table, the from-scratch SMT rebuild, the deterministic
//! rebuild backend, the validator-set derivation, and the offline inspector.
//!
//! These were the `a0` binary's private modules (gate F4, commits 5c5ff19,
//! 72e2317, c72601c). They live in the library now so one verifier can serve
//! every caller instead of each caller growing its own: the `a0` CLI stays a
//! thin wrapper over `audit::run`, `inspect::run` and `valset::print_valset`,
//! and the snapshot producer and installer call the same functions. One
//! verifier, one classification table, one identity. A second implementation
//! of any of these would be a second thing that can be wrong in a way the
//! first is not.
//!
//! This relocation is behaviour-neutral by construction: no check, no check
//! ordering, no report string, no exit code and no logic changed, only the
//! module path. The gate_a0_* suites drive the CLI binary end to end and pass
//! unmodified across the move.

pub mod audit;
pub mod bundle;
pub mod classify;
pub mod fetch;
pub mod inspect;
pub mod install;
pub mod produce;
pub mod producer;
pub mod rebuild;
pub mod reclaim;
pub mod stage;
pub mod store;
pub mod valset;
pub mod wire;
