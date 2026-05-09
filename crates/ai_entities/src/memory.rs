//! On-Chain Memory Objects for AI Entities (Week 21)
//!
//! PURPOSE: Provide AI entities with a native, public, deterministic memory substrate.
//! Memory objects allow AI to store and retrieve structured data on-chain.
//!
//! INVARIANTS:
//! - Object IDs are deterministically computed from content
//! - All encoding is canonical (big-endian, versioned)
//! - Object data is opaque bytes (type-specific encoding)
//! - No floats, no nondeterministic behavior
//!
//! FAILURE MODES:
//! - Invalid object type byte → decode error
//! - Data exceeds MAX_MEMORY_OBJECT_SIZE → validation error
//! - Entity exceeds MAX_MEMORY_OBJECTS_PER_ENTITY → validation error

use blake3::Hasher;

/// Domain separator for memory object ID computation.
const MEMORY_OBJECT_ID_DOMAIN: &[u8] = b"NOVAI_MEMORY_OBJECT_ID_V1";

/// Memory object encoding version.
pub const MEMORY_OBJECT_CODEC_V1: u8 = 1;

/// Maximum size of a single memory object's data field (64KB).
pub const MAX_MEMORY_OBJECT_SIZE: usize = 65536;

/// Maximum number of memory objects per AI entity.
pub const MAX_MEMORY_OBJECTS_PER_ENTITY: u32 = 100;

// ============================================================================
// MEMORY OBJECT TYPE ENUM (D21.1)
// ============================================================================

/// Type of memory object stored by an AI entity.
///
/// Each type has specific semantics for how the data field is interpreted:
/// - `ChainSummary`: Epoch summaries (block ranges, tx counts, fee totals)
/// - `LabelIndex`: Tags/categories for addresses
/// - `EmbeddingCommitment`: Hash of embedding vector (vector stored off-chain)
/// - `AnomalyLog`: Historical anomaly records
/// - `StatisticsSnapshot`: Periodic chain statistics
/// - `ReputationEvent`: Audit record of a reputation change applied to an entity
/// - `Rating`: A counterparty rating event feeding reputation
/// - `SignalCatalog`: Per-entity catalog of priced signal offerings for the
///   marketplace; payload is the canonical `SignalCatalogData` encoding
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MemoryObjectType {
    /// Epoch summaries containing block ranges, transaction counts, fee totals.
    #[default]
    ChainSummary = 0,
    /// Tags and categories for addresses (label → address mappings).
    LabelIndex = 1,
    /// Commitment hash of an embedding vector (actual vector stored off-chain).
    EmbeddingCommitment = 2,
    /// Historical record of detected anomalies.
    AnomalyLog = 3,
    /// Periodic snapshot of chain statistics.
    StatisticsSnapshot = 4,
    /// Audit record of a reputation change applied to an entity.
    ReputationEvent = 5,
    /// Counterparty rating (score, optional comment hash) feeding reputation.
    Rating = 6,
    /// Marketplace pricing catalog: a list of priced signal offerings the
    /// owning entity makes available to buyers.
    SignalCatalog = 7,
}

impl MemoryObjectType {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::ChainSummary),
            1 => Some(Self::LabelIndex),
            2 => Some(Self::EmbeddingCommitment),
            3 => Some(Self::AnomalyLog),
            4 => Some(Self::StatisticsSnapshot),
            5 => Some(Self::ReputationEvent),
            6 => Some(Self::Rating),
            7 => Some(Self::SignalCatalog),
            _ => None,
        }
    }

    /// Get human-readable name for this type.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ChainSummary => "ChainSummary",
            Self::LabelIndex => "LabelIndex",
            Self::EmbeddingCommitment => "EmbeddingCommitment",
            Self::AnomalyLog => "AnomalyLog",
            Self::StatisticsSnapshot => "StatisticsSnapshot",
            Self::ReputationEvent => "ReputationEvent",
            Self::Rating => "Rating",
            Self::SignalCatalog => "SignalCatalog",
        }
    }
}

// ============================================================================
// MEMORY OBJECT STRUCT (D21.2)
// ============================================================================

/// A memory object stored on-chain by an AI entity.
///
/// Memory objects provide persistent, public storage for AI entities to record
/// observations, summaries, and derived data. The `data` field contains
/// type-specific encoded content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryObject {
    /// Unique identifier: blake3(domain || owner || type || created_at || data_hash).
    pub object_id: [u8; 32],
    /// Type of this memory object.
    pub object_type: MemoryObjectType,
    /// AI entity that owns this object.
    pub owner_entity: [u8; 32],
    /// Block height when object was created.
    pub created_at: u64,
    /// Block height when object was last updated.
    pub updated_at: u64,
    /// Type-specific encoded data (max 64KB).
    pub data: Vec<u8>,
}

impl MemoryObject {
    /// Compute the canonical object ID from its components.
    ///
    /// The ID is deterministically derived from:
    /// - Domain separator
    /// - Owner entity ID
    /// - Object type
    /// - Creation timestamp
    /// - Hash of initial data
    #[must_use]
    pub fn compute_id(
        owner_entity: &[u8; 32],
        object_type: MemoryObjectType,
        created_at: u64,
        data: &[u8],
    ) -> [u8; 32] {
        let data_hash = blake3::hash(data);

        let mut hasher = Hasher::new();
        hasher.update(MEMORY_OBJECT_ID_DOMAIN);
        hasher.update(owner_entity);
        hasher.update(&[object_type.to_byte()]);
        hasher.update(&created_at.to_be_bytes());
        hasher.update(data_hash.as_bytes());
        *hasher.finalize().as_bytes()
    }

    /// Create a new memory object with computed ID.
    ///
    /// # Arguments
    /// - `owner_entity`: AI entity that owns this object
    /// - `object_type`: Type of memory object
    /// - `created_at`: Block height of creation
    /// - `data`: Initial data (must be ≤ MAX_MEMORY_OBJECT_SIZE)
    #[must_use]
    pub fn new(
        owner_entity: [u8; 32],
        object_type: MemoryObjectType,
        created_at: u64,
        data: Vec<u8>,
    ) -> Self {
        let object_id = Self::compute_id(&owner_entity, object_type, created_at, &data);
        Self {
            object_id,
            object_type,
            owner_entity,
            created_at,
            updated_at: created_at,
            data,
        }
    }

    /// Check if data size is within limits.
    #[must_use]
    pub fn is_valid_size(&self) -> bool {
        self.data.len() <= MAX_MEMORY_OBJECT_SIZE
    }

    /// Get the size of the data field.
    #[must_use]
    pub fn data_size(&self) -> usize {
        self.data.len()
    }
}

// ============================================================================
// ENCODING / DECODING
// ============================================================================

/// Error type for memory object decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryObjectDecodeError {
    /// Input too short for fixed fields.
    UnexpectedEof { expected: usize, got: usize },
    /// Invalid codec version.
    BadVersion { expected: u8, got: u8 },
    /// Invalid object type byte.
    InvalidObjectType { byte: u8 },
    /// Data length exceeds maximum.
    DataTooLarge { size: usize, max: usize },
}

/// Encode a `MemoryObject` to canonical bytes.
///
/// Format: `[version:1][object_id:32][object_type:1][owner_entity:32]`
///         `[created_at_be:8][updated_at_be:8][data_len_be:4][data:var]`
///
/// Fixed header: 86 bytes + variable data
#[must_use]
pub fn encode_memory_object_v1(obj: &MemoryObject) -> Vec<u8> {
    let data_len = obj.data.len();
    let total_len = 1 + 32 + 1 + 32 + 8 + 8 + 4 + data_len;
    let mut out = Vec::with_capacity(total_len);

    // Version
    out.push(MEMORY_OBJECT_CODEC_V1);

    // Object ID
    out.extend_from_slice(&obj.object_id);

    // Object type
    out.push(obj.object_type.to_byte());

    // Owner entity
    out.extend_from_slice(&obj.owner_entity);

    // Created at (big-endian)
    out.extend_from_slice(&obj.created_at.to_be_bytes());

    // Updated at (big-endian)
    out.extend_from_slice(&obj.updated_at.to_be_bytes());

    // Data length (big-endian u32)
    #[allow(clippy::cast_possible_truncation)]
    let data_len_u32 = data_len as u32;
    out.extend_from_slice(&data_len_u32.to_be_bytes());

    // Data
    out.extend_from_slice(&obj.data);

    out
}

/// Decode a `MemoryObject` from canonical bytes.
///
/// # Errors
/// Returns error if bytes are malformed or invalid.
pub fn decode_memory_object_v1(bytes: &[u8]) -> Result<MemoryObject, MemoryObjectDecodeError> {
    const HEADER_LEN: usize = 1 + 32 + 1 + 32 + 8 + 8 + 4; // 86 bytes

    if bytes.len() < HEADER_LEN {
        return Err(MemoryObjectDecodeError::UnexpectedEof {
            expected: HEADER_LEN,
            got: bytes.len(),
        });
    }

    let mut pos = 0;

    // Version
    let version = bytes[pos];
    if version != MEMORY_OBJECT_CODEC_V1 {
        return Err(MemoryObjectDecodeError::BadVersion {
            expected: MEMORY_OBJECT_CODEC_V1,
            got: version,
        });
    }
    pos += 1;

    // Object ID
    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Object type
    let object_type = MemoryObjectType::from_byte(bytes[pos])
        .ok_or(MemoryObjectDecodeError::InvalidObjectType { byte: bytes[pos] })?;
    pos += 1;

    // Owner entity
    let mut owner_entity = [0u8; 32];
    owner_entity.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Created at
    let mut created_at_bytes = [0u8; 8];
    created_at_bytes.copy_from_slice(&bytes[pos..pos + 8]);
    let created_at = u64::from_be_bytes(created_at_bytes);
    pos += 8;

    // Updated at
    let mut updated_at_bytes = [0u8; 8];
    updated_at_bytes.copy_from_slice(&bytes[pos..pos + 8]);
    let updated_at = u64::from_be_bytes(updated_at_bytes);
    pos += 8;

    // Data length
    let mut data_len_bytes = [0u8; 4];
    data_len_bytes.copy_from_slice(&bytes[pos..pos + 4]);
    let data_len = u32::from_be_bytes(data_len_bytes) as usize;
    pos += 4;

    // Validate data length
    if data_len > MAX_MEMORY_OBJECT_SIZE {
        return Err(MemoryObjectDecodeError::DataTooLarge {
            size: data_len,
            max: MAX_MEMORY_OBJECT_SIZE,
        });
    }

    // Check remaining bytes
    if bytes.len() < pos + data_len {
        return Err(MemoryObjectDecodeError::UnexpectedEof {
            expected: pos + data_len,
            got: bytes.len(),
        });
    }

    // Data
    let data = bytes[pos..pos + data_len].to_vec();

    Ok(MemoryObject {
        object_id,
        object_type,
        owner_entity,
        created_at,
        updated_at,
        data,
    })
}

// ============================================================================
// TYPE-SPECIFIC DATA STRUCTURES
// ============================================================================

/// Chain summary data for `MemoryObjectType::ChainSummary`.
///
/// Stores epoch-level statistics about a range of blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSummaryData {
    /// First block height in this summary.
    pub start_height: u64,
    /// Last block height in this summary.
    pub end_height: u64,
    /// Total number of transactions in range.
    pub tx_count: u64,
    /// Total fees collected in range.
    pub fee_total: u64,
    /// Average block fullness percentage (0-100).
    pub avg_block_fullness: u8,
}

impl ChainSummaryData {
    /// Encode to bytes for storage in MemoryObject.data.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(33);
        out.extend_from_slice(&self.start_height.to_be_bytes());
        out.extend_from_slice(&self.end_height.to_be_bytes());
        out.extend_from_slice(&self.tx_count.to_be_bytes());
        out.extend_from_slice(&self.fee_total.to_be_bytes());
        out.push(self.avg_block_fullness);
        out
    }

    /// Decode from bytes.
    ///
    /// # Errors
    /// Returns None if bytes are insufficient.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 33 {
            return None;
        }

        let start_height = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let end_height = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let tx_count = u64::from_be_bytes(bytes[16..24].try_into().ok()?);
        let fee_total = u64::from_be_bytes(bytes[24..32].try_into().ok()?);
        let avg_block_fullness = bytes[32];

        Some(Self {
            start_height,
            end_height,
            tx_count,
            fee_total,
            avg_block_fullness,
        })
    }
}

/// Statistics snapshot data for `MemoryObjectType::StatisticsSnapshot`.
///
/// Stores a point-in-time snapshot of chain statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatisticsSnapshotData {
    /// Block height at snapshot time.
    pub height: u64,
    /// Current mempool size.
    pub mempool_size: u64,
    /// Average fee over recent window.
    pub avg_fee: u64,
    /// P95 fee over recent window.
    pub fee_p95: u64,
    /// Number of active validators.
    pub validator_count: u32,
    /// Average block fullness percentage (0-100).
    pub avg_block_fullness: u8,
}

impl StatisticsSnapshotData {
    /// Encode to bytes for storage in MemoryObject.data.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(37);
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.mempool_size.to_be_bytes());
        out.extend_from_slice(&self.avg_fee.to_be_bytes());
        out.extend_from_slice(&self.fee_p95.to_be_bytes());
        out.extend_from_slice(&self.validator_count.to_be_bytes());
        out.push(self.avg_block_fullness);
        out
    }

    /// Decode from bytes.
    ///
    /// # Errors
    /// Returns None if bytes are insufficient.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 37 {
            return None;
        }

        let height = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let mempool_size = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let avg_fee = u64::from_be_bytes(bytes[16..24].try_into().ok()?);
        let fee_p95 = u64::from_be_bytes(bytes[24..32].try_into().ok()?);
        let validator_count = u32::from_be_bytes(bytes[32..36].try_into().ok()?);
        let avg_block_fullness = bytes[36];

        Some(Self {
            height,
            mempool_size,
            avg_fee,
            fee_p95,
            validator_count,
            avg_block_fullness,
        })
    }
}

// ============================================================================
// SIGNAL CATALOG (marketplace pricing)
// ============================================================================

/// Maximum number of distinct signal offerings a single entity may list.
pub const MAX_CATALOG_OFFERINGS: usize = 10;

/// On-wire size of one `SignalCatalogEntry` (signal_type + price_be + is_active).
pub const SIGNAL_CATALOG_ENTRY_SIZE: usize = 1 + 8 + 1;

/// Maximum encoded size of a full `SignalCatalogData` payload
/// (`1` count byte + up to `MAX_CATALOG_OFFERINGS` × `SIGNAL_CATALOG_ENTRY_SIZE`).
pub const SIGNAL_CATALOG_MAX_SIZE: usize = 1 + MAX_CATALOG_OFFERINGS * SIGNAL_CATALOG_ENTRY_SIZE;

/// One priced offering within a `SignalCatalog`.
///
/// Layout (10 bytes): `signal_type:1 | price_per_signal_be:8 | is_active:1`.
/// `is_active` is encoded as `0` (inactive) or `1` (active); decoders
/// reject any other value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalCatalogEntry {
    /// `AiSignalType` byte value this offering prices.
    pub signal_type: u8,
    /// Price per signal in the smallest token unit, big-endian on the wire.
    pub price_per_signal: u64,
    /// Whether this offering currently accepts purchases.
    pub is_active: bool,
}

/// A seller's catalog of priced signal offerings, stored as the data field of
/// a `MemoryObjectType::SignalCatalog` memory object.
///
/// On-wire layout: `count:1 | entries: count * SIGNAL_CATALOG_ENTRY_SIZE`,
/// where `count <= MAX_CATALOG_OFFERINGS`.
///
/// Catalog entries are not deduplicated by the codec; a buyer-side lookup is
/// expected to pick the first matching `signal_type`. Sellers wanting a
/// canonical view should ensure each `signal_type` appears at most once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignalCatalogData {
    /// Priced offerings (max `MAX_CATALOG_OFFERINGS`).
    pub entries: Vec<SignalCatalogEntry>,
}

impl SignalCatalogEntry {
    /// Encode this entry to its 10-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; SIGNAL_CATALOG_ENTRY_SIZE] {
        let mut out = [0u8; SIGNAL_CATALOG_ENTRY_SIZE];
        out[0] = self.signal_type;
        out[1..9].copy_from_slice(&self.price_per_signal.to_be_bytes());
        out[9] = u8::from(self.is_active);
        out
    }

    /// Decode a single entry from a 10-byte slice.
    ///
    /// Returns `None` if the slice is too short or `is_active` is not `0`/`1`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < SIGNAL_CATALOG_ENTRY_SIZE {
            return None;
        }
        let signal_type = bytes[0];
        let price_per_signal = u64::from_be_bytes(bytes[1..9].try_into().ok()?);
        let is_active = match bytes[9] {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Self {
            signal_type,
            price_per_signal,
            is_active,
        })
    }
}

impl SignalCatalogData {
    /// Encode this catalog to canonical bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let count = self.entries.len().min(MAX_CATALOG_OFFERINGS);
        let mut out = Vec::with_capacity(1 + count * SIGNAL_CATALOG_ENTRY_SIZE);
        #[allow(clippy::cast_possible_truncation)]
        out.push(count as u8);
        for entry in self.entries.iter().take(count) {
            out.extend_from_slice(&entry.encode());
        }
        out
    }

    /// Decode a catalog from canonical bytes.
    ///
    /// Returns `None` if the count byte exceeds `MAX_CATALOG_OFFERINGS`,
    /// the buffer is shorter than `1 + count * SIGNAL_CATALOG_ENTRY_SIZE`,
    /// or any entry fails to decode.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let count = bytes[0] as usize;
        if count > MAX_CATALOG_OFFERINGS {
            return None;
        }
        let expected_len = 1 + count * SIGNAL_CATALOG_ENTRY_SIZE;
        if bytes.len() < expected_len {
            return None;
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let off = 1 + i * SIGNAL_CATALOG_ENTRY_SIZE;
            let entry = SignalCatalogEntry::decode(&bytes[off..off + SIGNAL_CATALOG_ENTRY_SIZE])?;
            entries.push(entry);
        }
        Some(Self { entries })
    }

    /// Find the first offering matching `signal_type`, ignoring duplicates
    /// past the first match.
    #[must_use]
    pub fn find_offering(&self, signal_type: u8) -> Option<&SignalCatalogEntry> {
        self.entries.iter().find(|e| e.signal_type == signal_type)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_object_type_roundtrip() {
        for t in [
            MemoryObjectType::ChainSummary,
            MemoryObjectType::LabelIndex,
            MemoryObjectType::EmbeddingCommitment,
            MemoryObjectType::AnomalyLog,
            MemoryObjectType::StatisticsSnapshot,
            MemoryObjectType::ReputationEvent,
            MemoryObjectType::Rating,
            MemoryObjectType::SignalCatalog,
        ] {
            let byte = t.to_byte();
            let decoded = MemoryObjectType::from_byte(byte).unwrap();
            assert_eq!(t, decoded, "Type {t:?} roundtrip failed");
        }
    }

    #[test]
    fn memory_object_type_invalid_returns_none() {
        assert!(MemoryObjectType::from_byte(8).is_none());
        assert!(MemoryObjectType::from_byte(255).is_none());
    }

    #[test]
    fn memory_object_type_names() {
        assert_eq!(MemoryObjectType::ChainSummary.name(), "ChainSummary");
        assert_eq!(MemoryObjectType::LabelIndex.name(), "LabelIndex");
        assert_eq!(
            MemoryObjectType::EmbeddingCommitment.name(),
            "EmbeddingCommitment"
        );
        assert_eq!(MemoryObjectType::AnomalyLog.name(), "AnomalyLog");
        assert_eq!(
            MemoryObjectType::StatisticsSnapshot.name(),
            "StatisticsSnapshot"
        );
        assert_eq!(MemoryObjectType::ReputationEvent.name(), "ReputationEvent");
        assert_eq!(MemoryObjectType::Rating.name(), "Rating");
        assert_eq!(MemoryObjectType::SignalCatalog.name(), "SignalCatalog");
    }

    #[test]
    fn memory_object_id_is_deterministic() {
        let owner = [0x42u8; 32];
        let object_type = MemoryObjectType::ChainSummary;
        let created_at = 1000u64;
        let data = b"test data".to_vec();

        let id1 = MemoryObject::compute_id(&owner, object_type, created_at, &data);
        let id2 = MemoryObject::compute_id(&owner, object_type, created_at, &data);

        assert_eq!(id1, id2, "Object ID must be deterministic");
    }

    #[test]
    fn memory_object_id_changes_with_inputs() {
        let owner = [0x42u8; 32];
        let data = b"test data".to_vec();

        let id1 = MemoryObject::compute_id(&owner, MemoryObjectType::ChainSummary, 1000, &data);
        let id2 = MemoryObject::compute_id(&owner, MemoryObjectType::LabelIndex, 1000, &data);
        let id3 = MemoryObject::compute_id(&owner, MemoryObjectType::ChainSummary, 2000, &data);
        let id4 = MemoryObject::compute_id(&owner, MemoryObjectType::ChainSummary, 1000, b"other");

        assert_ne!(id1, id2, "Different type should produce different ID");
        assert_ne!(id1, id3, "Different created_at should produce different ID");
        assert_ne!(id1, id4, "Different data should produce different ID");
    }

    #[test]
    fn memory_object_new_computes_id() {
        let owner = [0x42u8; 32];
        let data = b"test data".to_vec();

        let obj = MemoryObject::new(owner, MemoryObjectType::ChainSummary, 1000, data.clone());

        let expected_id =
            MemoryObject::compute_id(&owner, MemoryObjectType::ChainSummary, 1000, &data);
        assert_eq!(obj.object_id, expected_id);
        assert_eq!(obj.created_at, 1000);
        assert_eq!(obj.updated_at, 1000);
    }

    #[test]
    fn memory_object_encode_decode_roundtrip() {
        let owner = [0x42u8; 32];
        let data = b"test memory data for roundtrip".to_vec();

        let obj = MemoryObject::new(owner, MemoryObjectType::AnomalyLog, 5000, data);

        let encoded = encode_memory_object_v1(&obj);
        let decoded = decode_memory_object_v1(&encoded).unwrap();

        assert_eq!(obj.object_id, decoded.object_id);
        assert_eq!(obj.object_type, decoded.object_type);
        assert_eq!(obj.owner_entity, decoded.owner_entity);
        assert_eq!(obj.created_at, decoded.created_at);
        assert_eq!(obj.updated_at, decoded.updated_at);
        assert_eq!(obj.data, decoded.data);
    }

    #[test]
    fn memory_object_decode_bad_version() {
        let mut bytes = vec![0u8; 100];
        bytes[0] = 99; // Invalid version

        let result = decode_memory_object_v1(&bytes);
        assert!(matches!(
            result,
            Err(MemoryObjectDecodeError::BadVersion {
                expected: 1,
                got: 99
            })
        ));
    }

    #[test]
    fn memory_object_decode_bad_type() {
        let owner = [0x42u8; 32];
        let obj = MemoryObject::new(owner, MemoryObjectType::ChainSummary, 1000, vec![]);

        let mut encoded = encode_memory_object_v1(&obj);
        encoded[33] = 99; // Invalid object type at position 33

        let result = decode_memory_object_v1(&encoded);
        assert!(matches!(
            result,
            Err(MemoryObjectDecodeError::InvalidObjectType { byte: 99 })
        ));
    }

    #[test]
    fn memory_object_decode_too_short() {
        let bytes = vec![1u8; 50]; // Too short for header

        let result = decode_memory_object_v1(&bytes);
        assert!(matches!(
            result,
            Err(MemoryObjectDecodeError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn memory_object_is_valid_size() {
        let owner = [0x42u8; 32];

        // Valid size
        let small_obj =
            MemoryObject::new(owner, MemoryObjectType::ChainSummary, 1000, vec![0u8; 100]);
        assert!(small_obj.is_valid_size());

        // At limit
        let max_obj = MemoryObject::new(
            owner,
            MemoryObjectType::ChainSummary,
            1000,
            vec![0u8; MAX_MEMORY_OBJECT_SIZE],
        );
        assert!(max_obj.is_valid_size());

        // Over limit
        let over_obj = MemoryObject::new(
            owner,
            MemoryObjectType::ChainSummary,
            1000,
            vec![0u8; MAX_MEMORY_OBJECT_SIZE + 1],
        );
        assert!(!over_obj.is_valid_size());
    }

    #[test]
    fn chain_summary_data_roundtrip() {
        let summary = ChainSummaryData {
            start_height: 100,
            end_height: 200,
            tx_count: 5000,
            fee_total: 123456,
            avg_block_fullness: 75,
        };

        let encoded = summary.encode();
        let decoded = ChainSummaryData::decode(&encoded).unwrap();

        assert_eq!(summary.start_height, decoded.start_height);
        assert_eq!(summary.end_height, decoded.end_height);
        assert_eq!(summary.tx_count, decoded.tx_count);
        assert_eq!(summary.fee_total, decoded.fee_total);
        assert_eq!(summary.avg_block_fullness, decoded.avg_block_fullness);
    }

    #[test]
    fn statistics_snapshot_data_roundtrip() {
        let snapshot = StatisticsSnapshotData {
            height: 10000,
            mempool_size: 250,
            avg_fee: 100,
            fee_p95: 500,
            validator_count: 21,
            avg_block_fullness: 60,
        };

        let encoded = snapshot.encode();
        let decoded = StatisticsSnapshotData::decode(&encoded).unwrap();

        assert_eq!(snapshot.height, decoded.height);
        assert_eq!(snapshot.mempool_size, decoded.mempool_size);
        assert_eq!(snapshot.avg_fee, decoded.avg_fee);
        assert_eq!(snapshot.fee_p95, decoded.fee_p95);
        assert_eq!(snapshot.validator_count, decoded.validator_count);
        assert_eq!(snapshot.avg_block_fullness, decoded.avg_block_fullness);
    }

    #[test]
    fn chain_summary_data_decode_too_short() {
        let bytes = vec![0u8; 10]; // Too short
        assert!(ChainSummaryData::decode(&bytes).is_none());
    }

    #[test]
    fn statistics_snapshot_data_decode_too_short() {
        let bytes = vec![0u8; 20]; // Too short
        assert!(StatisticsSnapshotData::decode(&bytes).is_none());
    }

    #[test]
    fn memory_object_all_types_encode_decode() {
        let owner = [0x42u8; 32];

        for object_type in [
            MemoryObjectType::ChainSummary,
            MemoryObjectType::LabelIndex,
            MemoryObjectType::EmbeddingCommitment,
            MemoryObjectType::AnomalyLog,
            MemoryObjectType::StatisticsSnapshot,
            MemoryObjectType::ReputationEvent,
            MemoryObjectType::Rating,
            MemoryObjectType::SignalCatalog,
        ] {
            let obj = MemoryObject::new(owner, object_type, 1000, b"type test".to_vec());
            let encoded = encode_memory_object_v1(&obj);
            let decoded = decode_memory_object_v1(&encoded).unwrap();

            assert_eq!(
                obj.object_type, decoded.object_type,
                "Type {object_type:?} failed"
            );
        }
    }

    #[test]
    fn encoding_is_deterministic() {
        let owner = [0x42u8; 32];
        let obj = MemoryObject::new(
            owner,
            MemoryObjectType::ChainSummary,
            1000,
            b"test".to_vec(),
        );

        let enc1 = encode_memory_object_v1(&obj);
        let enc2 = encode_memory_object_v1(&obj);

        assert_eq!(enc1, enc2, "Encoding must be deterministic");
    }

    // ========================================================================
    // SignalCatalog tests
    // ========================================================================

    #[test]
    fn signal_catalog_entry_byte_layout() {
        let entry = SignalCatalogEntry {
            signal_type: 3,
            price_per_signal: 0x0102_0304_0506_0708,
            is_active: true,
        };
        let encoded = entry.encode();

        assert_eq!(encoded.len(), SIGNAL_CATALOG_ENTRY_SIZE);
        assert_eq!(encoded[0], 3, "signal_type at offset 0");
        assert_eq!(
            &encoded[1..9],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "price_be at offsets 1..9"
        );
        assert_eq!(encoded[9], 1, "is_active=true encodes as 1");
    }

    #[test]
    fn signal_catalog_entry_roundtrip() {
        let cases = [
            SignalCatalogEntry {
                signal_type: 0,
                price_per_signal: 0,
                is_active: false,
            },
            SignalCatalogEntry {
                signal_type: 7,
                price_per_signal: u64::MAX,
                is_active: true,
            },
            SignalCatalogEntry {
                signal_type: 5,
                price_per_signal: 1_000_000,
                is_active: false,
            },
        ];
        for entry in cases {
            let encoded = entry.encode();
            let decoded = SignalCatalogEntry::decode(&encoded).expect("decode");
            assert_eq!(entry, decoded, "roundtrip {entry:?}");
        }
    }

    #[test]
    fn signal_catalog_entry_decode_invalid_active_byte() {
        let mut bytes = [0u8; SIGNAL_CATALOG_ENTRY_SIZE];
        bytes[9] = 2;
        assert!(SignalCatalogEntry::decode(&bytes).is_none());
    }

    #[test]
    fn signal_catalog_entry_decode_too_short() {
        let bytes = [0u8; 9];
        assert!(SignalCatalogEntry::decode(&bytes).is_none());
    }

    #[test]
    fn signal_catalog_data_empty_roundtrip() {
        let cat = SignalCatalogData {
            entries: Vec::new(),
        };
        let encoded = cat.encode();
        assert_eq!(encoded, vec![0u8], "empty catalog is a single zero byte");
        let decoded = SignalCatalogData::decode(&encoded).expect("decode");
        assert_eq!(cat, decoded);
    }

    #[test]
    fn signal_catalog_data_single_entry_roundtrip() {
        let cat = SignalCatalogData {
            entries: vec![SignalCatalogEntry {
                signal_type: 2,
                price_per_signal: 42,
                is_active: true,
            }],
        };
        let encoded = cat.encode();
        assert_eq!(encoded.len(), 1 + SIGNAL_CATALOG_ENTRY_SIZE);
        assert_eq!(encoded[0], 1, "count byte");
        let decoded = SignalCatalogData::decode(&encoded).expect("decode");
        assert_eq!(cat, decoded);
    }

    #[test]
    fn signal_catalog_data_full_capacity_roundtrip() {
        let entries: Vec<SignalCatalogEntry> = (0..MAX_CATALOG_OFFERINGS)
            .map(|i| SignalCatalogEntry {
                signal_type: i as u8,
                price_per_signal: 100 * (i as u64 + 1),
                is_active: i % 2 == 0,
            })
            .collect();
        let cat = SignalCatalogData { entries };
        let encoded = cat.encode();
        assert_eq!(encoded.len(), SIGNAL_CATALOG_MAX_SIZE);
        assert_eq!(encoded.len(), 101, "max catalog encoding is 101 bytes");
        let decoded = SignalCatalogData::decode(&encoded).expect("decode");
        assert_eq!(cat, decoded);
    }

    #[test]
    fn signal_catalog_data_decode_count_too_large() {
        let mut bytes = vec![0u8; 1 + (MAX_CATALOG_OFFERINGS + 1) * SIGNAL_CATALOG_ENTRY_SIZE];
        bytes[0] = (MAX_CATALOG_OFFERINGS + 1) as u8;
        assert!(SignalCatalogData::decode(&bytes).is_none());
    }

    #[test]
    fn signal_catalog_data_decode_truncated() {
        // Claims 2 entries but only carries 1 entry's worth of bytes.
        let mut bytes = vec![0u8; 1 + SIGNAL_CATALOG_ENTRY_SIZE];
        bytes[0] = 2;
        assert!(SignalCatalogData::decode(&bytes).is_none());
    }

    #[test]
    fn signal_catalog_data_decode_empty_buffer() {
        assert!(SignalCatalogData::decode(&[]).is_none());
    }

    #[test]
    fn signal_catalog_data_encode_caps_at_max_offerings() {
        // If a caller hands in more than MAX_CATALOG_OFFERINGS, encode truncates
        // rather than producing an oversized payload that decode would reject.
        let entries: Vec<SignalCatalogEntry> = (0..MAX_CATALOG_OFFERINGS + 5)
            .map(|i| SignalCatalogEntry {
                signal_type: i as u8,
                price_per_signal: 1,
                is_active: true,
            })
            .collect();
        let cat = SignalCatalogData { entries };
        let encoded = cat.encode();
        assert_eq!(encoded.len(), SIGNAL_CATALOG_MAX_SIZE);
        assert_eq!(encoded[0] as usize, MAX_CATALOG_OFFERINGS);
    }

    #[test]
    fn signal_catalog_find_offering_picks_first_match() {
        let cat = SignalCatalogData {
            entries: vec![
                SignalCatalogEntry {
                    signal_type: 4,
                    price_per_signal: 100,
                    is_active: true,
                },
                SignalCatalogEntry {
                    signal_type: 4,
                    price_per_signal: 999,
                    is_active: false,
                },
            ],
        };
        let hit = cat.find_offering(4).expect("found");
        assert_eq!(
            hit.price_per_signal, 100,
            "find_offering returns first match"
        );
        assert!(cat.find_offering(7).is_none());
    }
}
