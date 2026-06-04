//! Transaction generator library.
//!
//! This crate is primarily a binary (`tx-generator`) for load testing
//! NOVAI nodes, but the building blocks are exposed as a library so
//! integration tests under `tests/` can drive `Generator` + `Submitter`
//! against a mock RPC endpoint without spawning the CLI process.

pub mod generator;
pub mod metrics;
pub mod sender;
pub mod submitter;
