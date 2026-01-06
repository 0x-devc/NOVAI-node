//! novai-smt
//!
//! Purpose (Week 4):
//! - Provide a clean-room, deterministic Sparse Merkle Tree (SMT) suitable for
//!   consensus-critical state roots.
//!
//! High-level model:
//! - Key space: 256-bit (32-byte) keys.
//! - Path: bits of key, MSB-first (bit 0 is highest bit of byte[0]).
//! - Leaf value: hash(value_bytes) with domain separation.
//! - Internal nodes: hash(left_child || right_child) with domain separation.
//!
//! Invariants:
//! - No nondeterminism (no floats, no reliance on iteration order).
//! - Hashing is canonical and domain-separated.
//! - Empty hash rules are fixed and tested.
//!
//! Failure modes:
//! - Store I/O errors must be surfaced (never ignored).
//! - Bad node encodings must be rejected (never interpreted loosely).
//!
//! NOTE: This crate is intentionally minimal and dependency-light.

pub mod hash;
pub mod node;
pub mod smt;

pub use hash::{empty_hash_at_height, hash_internal, hash_leaf, Hash32};
pub use node::{Node, NodeChild, NodeEncodingError};
pub use smt::{MemoryStore, Smt, SmtError, SmtStore};
