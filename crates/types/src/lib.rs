//! Protocol types.
//!
//! IMPORTANT:
//! - These structs participate in consensus/networking.
//! - Changing field order or encoding is a hard-fork unless you bump `version`.
//! - Avoid HashMap/iteration-order-dependent structures in consensus-relevant types.

pub type Address = [u8; 32]; // For Week 2: ed25519 public key bytes.
pub type TxId = [u8; 32];
pub type Hash32 = [u8; 32];
pub type Nonce = u64;
pub type Fee = u64;

/// V1 signature: raw ed25519 signature bytes (64 bytes).
pub type SignatureBytes = [u8; 64];

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxVersion {
    V1 = 1,
}

impl TxVersion {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(TxVersion::V1),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockHeaderVersion {
    V1 = 1,
}

impl BlockHeaderVersion {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(BlockHeaderVersion::V1),
            _ => None,
        }
    }
}

/// Canonical V1 transaction.
///
/// Signing rule (Week 2):
/// - Signature is computed over the canonical *unsigned* encoding of this tx
///   (everything except `sig`).
/// - `from` is the 32-byte ed25519 public key (Address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxV1 {
    pub version: TxVersion,
    pub from: Address,
    pub nonce: Nonce,
    pub fee: Fee,
    pub payload: Vec<u8>,
    pub sig: SignatureBytes,
}

/// Canonical V1 block header (even if blocks are not produced yet).
///
/// Notes:
/// - All hashes are 32 bytes.
/// - `qc_hash` is a placeholder for Week 2 (still fixed-size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeaderV1 {
    pub version: BlockHeaderVersion,
    pub height: u64,
    pub prev_hash: Hash32,
    pub state_root: Hash32,
    pub tx_root: Hash32,
    pub proposer: Address,
    pub qc_hash: Hash32,
}
