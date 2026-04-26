//! Derived Views for AI Privacy Bridge (Week 23)
//!
//! PURPOSE: Define privacy-preserving derived outputs that AI entities can read
//! without accessing raw private data. Derived views provide aggregated,
//! schema-validated information from private data.
//!
//! INVARIANTS:
//! - View IDs are deterministically computed from content
//! - All encoding is canonical (big-endian, versioned)
//! - Data must conform to registered schema
//! - No raw private data exposed (only aggregates)
//!
//! FAILURE MODES:
//! - Invalid source type byte → decode error
//! - Invalid schema ID → decode error
//! - Data exceeds MAX_DERIVED_VIEW_SIZE → validation error
//! - Schema validation fails → creation error

use blake3::Hasher;

/// Domain separator for derived view ID computation.
const DERIVED_VIEW_ID_DOMAIN: &[u8] = b"NOVAI_DERIVED_VIEW_ID_V1";

/// Derived view encoding version.
pub const DERIVED_VIEW_CODEC_V1: u8 = 1;

/// Maximum size of a derived view's data field (16KB).
/// Smaller than memory objects since these are aggregates.
pub const MAX_DERIVED_VIEW_SIZE: usize = 16384;

// ============================================================================
// DERIVED SOURCE TYPE (D23.3)
// ============================================================================

/// How the derived view was generated.
///
/// This determines the trust model and access permissions:
/// - `ChainAggregate`: Automatically generated from on-chain public data
/// - `UserAuthorized`: User explicitly permitted this derivation
/// - `ProtocolGenerated`: Protocol-level automatic derivation (e.g., pool stats)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DerivedSourceType {
    /// Aggregate computed from public on-chain data.
    /// No privacy concerns - derived from already-public information.
    #[default]
    ChainAggregate = 0,

    /// User explicitly authorized this derivation from their private data.
    /// Requires user signature/consent recorded on-chain.
    UserAuthorized = 1,

    /// Protocol automatically generates this view (e.g., shielded pool size).
    /// Designed to not leak individual transaction information.
    ProtocolGenerated = 2,
}

impl DerivedSourceType {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::ChainAggregate),
            1 => Some(Self::UserAuthorized),
            2 => Some(Self::ProtocolGenerated),
            _ => None,
        }
    }

    /// Get human-readable name for this source type.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ChainAggregate => "ChainAggregate",
            Self::UserAuthorized => "UserAuthorized",
            Self::ProtocolGenerated => "ProtocolGenerated",
        }
    }
}

// ============================================================================
// DERIVED VIEW SCHEMA (D23.2)
// ============================================================================

/// Predefined schemas for derived views.
///
/// Each schema defines the structure and semantics of the data field.
/// New schemas can be added via governance.
///
/// # Schema Definitions
///
/// - `AggregateVolume` (ID 1): Total transaction volume in a time window
/// - `ActivityCount` (ID 2): Number of transactions (not per-address)
/// - `PoolSize` (ID 3): Total value in shielded pool
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DerivedViewSchema {
    /// Schema 1: Total transaction volume in a time window.
    ///
    /// Data format: `[start_height:8][end_height:8][total_volume:16]`
    /// Total: 32 bytes
    #[default]
    AggregateVolume = 1,

    /// Schema 2: Number of transactions in a time window (not per-address).
    ///
    /// Data format: `[start_height:8][end_height:8][tx_count:8]`
    /// Total: 24 bytes
    ActivityCount = 2,

    /// Schema 3: Total value currently in the shielded pool.
    ///
    /// Data format: `[snapshot_height:8][pool_size:16]`
    /// Total: 24 bytes
    PoolSize = 3,
}

impl DerivedViewSchema {
    /// Encode to canonical byte representation (schema ID).
    #[must_use]
    pub const fn to_id(self) -> u32 {
        self as u32
    }

    /// Decode from schema ID, returning None for invalid/unknown schemas.
    #[must_use]
    pub const fn from_id(id: u32) -> Option<Self> {
        match id {
            1 => Some(Self::AggregateVolume),
            2 => Some(Self::ActivityCount),
            3 => Some(Self::PoolSize),
            _ => None,
        }
    }

    /// Get human-readable name for this schema.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::AggregateVolume => "AggregateVolume",
            Self::ActivityCount => "ActivityCount",
            Self::PoolSize => "PoolSize",
        }
    }

    /// Get the expected data length for this schema.
    ///
    /// Returns None if schema allows variable-length data (none currently do).
    #[must_use]
    pub const fn expected_data_len(&self) -> Option<usize> {
        match self {
            Self::AggregateVolume => Some(32), // 8 + 8 + 16
            Self::ActivityCount => Some(24),   // 8 + 8 + 8
            Self::PoolSize => Some(24),        // 8 + 16
        }
    }

    /// Validate that data conforms to this schema's expected format.
    #[must_use]
    pub fn validate_data(&self, data: &[u8]) -> bool {
        match self.expected_data_len() {
            Some(expected) => data.len() == expected,
            None => data.len() <= MAX_DERIVED_VIEW_SIZE,
        }
    }
}

// ============================================================================
// DERIVED VIEW STRUCT (D23.1)
// ============================================================================

/// A derived view providing privacy-safe aggregate data.
///
/// Derived views allow AI entities to read aggregate information derived from
/// private data without exposing individual records. Each view:
/// - Has a deterministic ID based on content
/// - Is validated against a registered schema
/// - Tracks its source (how it was created)
/// - Is bounded in size
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedView {
    /// Unique identifier: blake3(domain || source_type || schema_id || created_at || creator || data_hash).
    pub view_id: [u8; 32],

    /// How this view was generated.
    pub source_type: DerivedSourceType,

    /// Schema that defines the data format.
    pub schema_id: u32,

    /// Block height when view was created.
    pub created_at: u64,

    /// Address/entity that created this view.
    pub creator: [u8; 32],

    /// Schema-validated data (max 16KB).
    pub data: Vec<u8>,
}

impl DerivedView {
    /// Compute the canonical view ID from its components.
    ///
    /// The ID is deterministically derived from:
    /// - Domain separator
    /// - Source type
    /// - Schema ID
    /// - Creation timestamp
    /// - Creator address
    /// - Hash of data
    #[must_use]
    pub fn compute_id(
        source_type: DerivedSourceType,
        schema_id: u32,
        created_at: u64,
        creator: &[u8; 32],
        data: &[u8],
    ) -> [u8; 32] {
        let data_hash = blake3::hash(data);

        let mut hasher = Hasher::new();
        hasher.update(DERIVED_VIEW_ID_DOMAIN);
        hasher.update(&[source_type.to_byte()]);
        hasher.update(&schema_id.to_be_bytes());
        hasher.update(&created_at.to_be_bytes());
        hasher.update(creator);
        hasher.update(data_hash.as_bytes());
        *hasher.finalize().as_bytes()
    }

    /// Create a new derived view with computed ID.
    ///
    /// # Arguments
    /// - `source_type`: How the view was generated
    /// - `schema_id`: Schema that validates the data
    /// - `created_at`: Block height of creation
    /// - `creator`: Address/entity that created this view
    /// - `data`: Schema-validated data (must be ≤ MAX_DERIVED_VIEW_SIZE)
    ///
    /// # Returns
    /// `Some(DerivedView)` if schema validation passes, `None` otherwise.
    #[must_use]
    pub fn new(
        source_type: DerivedSourceType,
        schema_id: u32,
        created_at: u64,
        creator: [u8; 32],
        data: Vec<u8>,
    ) -> Option<Self> {
        // Validate schema exists
        let schema = DerivedViewSchema::from_id(schema_id)?;

        // Validate data against schema
        if !schema.validate_data(&data) {
            return None;
        }

        // Validate size limit
        if data.len() > MAX_DERIVED_VIEW_SIZE {
            return None;
        }

        let view_id = Self::compute_id(source_type, schema_id, created_at, &creator, &data);

        Some(Self {
            view_id,
            source_type,
            schema_id,
            created_at,
            creator,
            data,
        })
    }

    /// Create a derived view without schema validation (for decoding).
    ///
    /// Used internally when decoding from storage where data was already validated.
    fn from_parts(
        view_id: [u8; 32],
        source_type: DerivedSourceType,
        schema_id: u32,
        created_at: u64,
        creator: [u8; 32],
        data: Vec<u8>,
    ) -> Self {
        Self {
            view_id,
            source_type,
            schema_id,
            created_at,
            creator,
            data,
        }
    }

    /// Get the schema for this view.
    #[must_use]
    pub fn schema(&self) -> Option<DerivedViewSchema> {
        DerivedViewSchema::from_id(self.schema_id)
    }

    /// Check if data size is within limits.
    #[must_use]
    pub fn is_valid_size(&self) -> bool {
        self.data.len() <= MAX_DERIVED_VIEW_SIZE
    }
}

// ============================================================================
// SCHEMA-SPECIFIC DATA STRUCTURES
// ============================================================================

/// Data for `DerivedViewSchema::AggregateVolume`.
///
/// Stores total transaction volume over a block range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateVolumeData {
    /// First block height in this aggregate.
    pub start_height: u64,
    /// Last block height in this aggregate.
    pub end_height: u64,
    /// Total volume (sum of all transaction values) in the range.
    pub total_volume: u128,
}

impl AggregateVolumeData {
    /// Encode to bytes for storage in DerivedView.data.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        out.extend_from_slice(&self.start_height.to_be_bytes());
        out.extend_from_slice(&self.end_height.to_be_bytes());
        out.extend_from_slice(&self.total_volume.to_be_bytes());
        out
    }

    /// Decode from bytes.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 32 {
            return None;
        }

        let start_height = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let end_height = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let total_volume = u128::from_be_bytes(bytes[16..32].try_into().ok()?);

        Some(Self {
            start_height,
            end_height,
            total_volume,
        })
    }
}

/// Data for `DerivedViewSchema::ActivityCount`.
///
/// Stores number of transactions over a block range (not per-address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityCountData {
    /// First block height in this count.
    pub start_height: u64,
    /// Last block height in this count.
    pub end_height: u64,
    /// Total number of transactions in the range.
    pub tx_count: u64,
}

impl ActivityCountData {
    /// Encode to bytes for storage in DerivedView.data.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&self.start_height.to_be_bytes());
        out.extend_from_slice(&self.end_height.to_be_bytes());
        out.extend_from_slice(&self.tx_count.to_be_bytes());
        out
    }

    /// Decode from bytes.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 24 {
            return None;
        }

        let start_height = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let end_height = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let tx_count = u64::from_be_bytes(bytes[16..24].try_into().ok()?);

        Some(Self {
            start_height,
            end_height,
            tx_count,
        })
    }
}

/// Data for `DerivedViewSchema::PoolSize`.
///
/// Stores total value in the shielded pool at a point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSizeData {
    /// Block height of this snapshot.
    pub snapshot_height: u64,
    /// Total value currently in the shielded pool.
    pub pool_size: u128,
}

impl PoolSizeData {
    /// Encode to bytes for storage in DerivedView.data.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&self.snapshot_height.to_be_bytes());
        out.extend_from_slice(&self.pool_size.to_be_bytes());
        out
    }

    /// Decode from bytes.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 24 {
            return None;
        }

        let snapshot_height = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let pool_size = u128::from_be_bytes(bytes[8..24].try_into().ok()?);

        Some(Self {
            snapshot_height,
            pool_size,
        })
    }
}

// ============================================================================
// PRIVACY BUDGET STUB (D23.4)
// ============================================================================

/// Privacy budget consumed by each derived view read.
///
/// This is a STUB - not enforced in Week 23.
/// Future weeks will implement budget tracking and enforcement.
pub const PRIVACY_BUDGET_PER_VIEW: u64 = 1;

/// Privacy budget replenishment rate (units per block).
///
/// This is a STUB - not enforced in Week 23.
pub const BUDGET_REPLENISH_RATE: u64 = 10;

/// Maximum privacy budget per entity.
///
/// This is a STUB - not enforced in Week 23.
pub const MAX_PRIVACY_BUDGET: u64 = 1000;

/// Privacy budget tracker (STUB - Week 23).
///
/// Tracks how much privacy budget an AI entity has consumed.
/// Budget replenishes over time to prevent unbounded information leakage.
///
/// # Future Implementation
///
/// This struct defines the interface but is NOT ENFORCED in Week 23.
/// Enforcement will be added in a future week.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrivacyBudget {
    /// Current available budget.
    pub available: u64,
    /// Total budget consumed (lifetime).
    pub consumed: u64,
    /// Last block height when budget was replenished.
    pub last_replenish_height: u64,
}

impl PrivacyBudget {
    /// Create a new privacy budget with maximum available.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            available: MAX_PRIVACY_BUDGET,
            consumed: 0,
            last_replenish_height: 0,
        }
    }

    /// Check if budget is available for a read (STUB - always returns true).
    ///
    /// In Week 23, this is not enforced - always returns true.
    #[must_use]
    pub const fn can_read(&self) -> bool {
        // STUB: Not enforced in Week 23
        true
    }

    /// Consume budget for a read (STUB - no-op in Week 23).
    ///
    /// In Week 23, this records the consumption but doesn't enforce limits.
    pub fn consume(&mut self, _amount: u64) {
        // STUB: Record but don't enforce in Week 23
        self.consumed = self.consumed.saturating_add(PRIVACY_BUDGET_PER_VIEW);
        self.available = self.available.saturating_sub(PRIVACY_BUDGET_PER_VIEW);
    }

    /// Replenish budget based on blocks elapsed (STUB - no-op in Week 23).
    ///
    /// In Week 23, this is a no-op.
    pub fn replenish(&mut self, _current_height: u64) {
        // STUB: No-op in Week 23
        // Future: available += (current_height - last_replenish_height) * BUDGET_REPLENISH_RATE
    }

    /// Encode to bytes for storage.
    #[must_use]
    pub fn encode(&self) -> [u8; 24] {
        let mut out = [0u8; 24];
        out[0..8].copy_from_slice(&self.available.to_be_bytes());
        out[8..16].copy_from_slice(&self.consumed.to_be_bytes());
        out[16..24].copy_from_slice(&self.last_replenish_height.to_be_bytes());
        out
    }

    /// Decode from bytes.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 24 {
            return None;
        }

        let available = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let consumed = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let last_replenish_height = u64::from_be_bytes(bytes[16..24].try_into().ok()?);

        Some(Self {
            available,
            consumed,
            last_replenish_height,
        })
    }
}

// ============================================================================
// ENCODING / DECODING
// ============================================================================

/// Error type for derived view decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedViewDecodeError {
    /// Input too short for fixed fields.
    UnexpectedEof { expected: usize, got: usize },
    /// Invalid codec version.
    BadVersion { expected: u8, got: u8 },
    /// Invalid source type byte.
    InvalidSourceType { byte: u8 },
    /// Invalid or unknown schema ID.
    InvalidSchemaId { id: u32 },
    /// Data length exceeds maximum.
    DataTooLarge { size: usize, max: usize },
}

/// Encode a `DerivedView` to canonical bytes.
///
/// Format: `[version:1][view_id:32][source_type:1][schema_id_be:4]`
///         `[created_at_be:8][creator:32][data_len_be:4][data:var]`
///
/// Fixed header: 82 bytes + variable data
#[must_use]
pub fn encode_derived_view_v1(view: &DerivedView) -> Vec<u8> {
    let data_len = view.data.len();
    let total_len = 1 + 32 + 1 + 4 + 8 + 32 + 4 + data_len;
    let mut out = Vec::with_capacity(total_len);

    // Version
    out.push(DERIVED_VIEW_CODEC_V1);

    // View ID
    out.extend_from_slice(&view.view_id);

    // Source type
    out.push(view.source_type.to_byte());

    // Schema ID (big-endian u32)
    out.extend_from_slice(&view.schema_id.to_be_bytes());

    // Created at (big-endian u64)
    out.extend_from_slice(&view.created_at.to_be_bytes());

    // Creator
    out.extend_from_slice(&view.creator);

    // Data length (big-endian u32)
    #[allow(clippy::cast_possible_truncation)]
    let data_len_u32 = data_len as u32;
    out.extend_from_slice(&data_len_u32.to_be_bytes());

    // Data
    out.extend_from_slice(&view.data);

    out
}

/// Decode a `DerivedView` from canonical bytes.
///
/// # Errors
/// Returns error if bytes are malformed or invalid.
pub fn decode_derived_view_v1(bytes: &[u8]) -> Result<DerivedView, DerivedViewDecodeError> {
    const HEADER_LEN: usize = 1 + 32 + 1 + 4 + 8 + 32 + 4; // 82 bytes

    if bytes.len() < HEADER_LEN {
        return Err(DerivedViewDecodeError::UnexpectedEof {
            expected: HEADER_LEN,
            got: bytes.len(),
        });
    }

    let mut pos = 0;

    // Version
    let version = bytes[pos];
    if version != DERIVED_VIEW_CODEC_V1 {
        return Err(DerivedViewDecodeError::BadVersion {
            expected: DERIVED_VIEW_CODEC_V1,
            got: version,
        });
    }
    pos += 1;

    // View ID
    let mut view_id = [0u8; 32];
    view_id.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Source type
    let source_type = DerivedSourceType::from_byte(bytes[pos])
        .ok_or(DerivedViewDecodeError::InvalidSourceType { byte: bytes[pos] })?;
    pos += 1;

    // Schema ID
    let mut schema_id_bytes = [0u8; 4];
    schema_id_bytes.copy_from_slice(&bytes[pos..pos + 4]);
    let schema_id = u32::from_be_bytes(schema_id_bytes);
    pos += 4;

    // Validate schema exists (but don't validate data - it was validated at creation)
    if DerivedViewSchema::from_id(schema_id).is_none() {
        return Err(DerivedViewDecodeError::InvalidSchemaId { id: schema_id });
    }

    // Created at
    let mut created_at_bytes = [0u8; 8];
    created_at_bytes.copy_from_slice(&bytes[pos..pos + 8]);
    let created_at = u64::from_be_bytes(created_at_bytes);
    pos += 8;

    // Creator
    let mut creator = [0u8; 32];
    creator.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Data length
    let mut data_len_bytes = [0u8; 4];
    data_len_bytes.copy_from_slice(&bytes[pos..pos + 4]);
    let data_len = u32::from_be_bytes(data_len_bytes) as usize;
    pos += 4;

    // Validate data length
    if data_len > MAX_DERIVED_VIEW_SIZE {
        return Err(DerivedViewDecodeError::DataTooLarge {
            size: data_len,
            max: MAX_DERIVED_VIEW_SIZE,
        });
    }

    // Check remaining bytes
    if bytes.len() < pos + data_len {
        return Err(DerivedViewDecodeError::UnexpectedEof {
            expected: pos + data_len,
            got: bytes.len(),
        });
    }

    // Data
    let data = bytes[pos..pos + data_len].to_vec();

    Ok(DerivedView::from_parts(
        view_id,
        source_type,
        schema_id,
        created_at,
        creator,
        data,
    ))
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // SOURCE TYPE TESTS
    // ========================================================================

    #[test]
    fn source_type_roundtrip() {
        for t in [
            DerivedSourceType::ChainAggregate,
            DerivedSourceType::UserAuthorized,
            DerivedSourceType::ProtocolGenerated,
        ] {
            let byte = t.to_byte();
            let decoded = DerivedSourceType::from_byte(byte).unwrap();
            assert_eq!(t, decoded, "Source type {t:?} roundtrip failed");
        }
    }

    #[test]
    fn source_type_invalid_returns_none() {
        assert!(DerivedSourceType::from_byte(3).is_none());
        assert!(DerivedSourceType::from_byte(255).is_none());
    }

    #[test]
    fn source_type_names() {
        assert_eq!(DerivedSourceType::ChainAggregate.name(), "ChainAggregate");
        assert_eq!(DerivedSourceType::UserAuthorized.name(), "UserAuthorized");
        assert_eq!(
            DerivedSourceType::ProtocolGenerated.name(),
            "ProtocolGenerated"
        );
    }

    // ========================================================================
    // SCHEMA TESTS
    // ========================================================================

    #[test]
    fn schema_roundtrip() {
        for s in [
            DerivedViewSchema::AggregateVolume,
            DerivedViewSchema::ActivityCount,
            DerivedViewSchema::PoolSize,
        ] {
            let id = s.to_id();
            let decoded = DerivedViewSchema::from_id(id).unwrap();
            assert_eq!(s, decoded, "Schema {s:?} roundtrip failed");
        }
    }

    #[test]
    fn schema_invalid_returns_none() {
        assert!(DerivedViewSchema::from_id(0).is_none());
        assert!(DerivedViewSchema::from_id(4).is_none());
        assert!(DerivedViewSchema::from_id(u32::MAX).is_none());
    }

    #[test]
    fn schema_names() {
        assert_eq!(DerivedViewSchema::AggregateVolume.name(), "AggregateVolume");
        assert_eq!(DerivedViewSchema::ActivityCount.name(), "ActivityCount");
        assert_eq!(DerivedViewSchema::PoolSize.name(), "PoolSize");
    }

    #[test]
    fn schema_expected_lengths() {
        assert_eq!(
            DerivedViewSchema::AggregateVolume.expected_data_len(),
            Some(32)
        );
        assert_eq!(
            DerivedViewSchema::ActivityCount.expected_data_len(),
            Some(24)
        );
        assert_eq!(DerivedViewSchema::PoolSize.expected_data_len(), Some(24));
    }

    #[test]
    fn schema_validates_data() {
        // AggregateVolume requires exactly 32 bytes
        assert!(DerivedViewSchema::AggregateVolume.validate_data(&[0u8; 32]));
        assert!(!DerivedViewSchema::AggregateVolume.validate_data(&[0u8; 31]));
        assert!(!DerivedViewSchema::AggregateVolume.validate_data(&[0u8; 33]));

        // ActivityCount requires exactly 24 bytes
        assert!(DerivedViewSchema::ActivityCount.validate_data(&[0u8; 24]));
        assert!(!DerivedViewSchema::ActivityCount.validate_data(&[0u8; 23]));

        // PoolSize requires exactly 24 bytes
        assert!(DerivedViewSchema::PoolSize.validate_data(&[0u8; 24]));
        assert!(!DerivedViewSchema::PoolSize.validate_data(&[0u8; 25]));
    }

    // ========================================================================
    // DERIVED VIEW TESTS
    // ========================================================================

    #[test]
    fn derived_view_id_is_deterministic() {
        let creator = [0x42u8; 32];
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        let id1 =
            DerivedView::compute_id(DerivedSourceType::ChainAggregate, 1, 1000, &creator, &data);
        let id2 =
            DerivedView::compute_id(DerivedSourceType::ChainAggregate, 1, 1000, &creator, &data);

        assert_eq!(id1, id2, "View ID must be deterministic");
    }

    #[test]
    fn derived_view_id_changes_with_inputs() {
        let creator = [0x42u8; 32];
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        let id1 =
            DerivedView::compute_id(DerivedSourceType::ChainAggregate, 1, 1000, &creator, &data);
        let id2 = DerivedView::compute_id(
            DerivedSourceType::UserAuthorized, // different source
            1,
            1000,
            &creator,
            &data,
        );
        let id3 = DerivedView::compute_id(
            DerivedSourceType::ChainAggregate,
            2, // different schema
            1000,
            &creator,
            &ActivityCountData {
                start_height: 100,
                end_height: 200,
                tx_count: 500,
            }
            .encode(),
        );

        assert_ne!(id1, id2, "Different source should produce different ID");
        assert_ne!(id1, id3, "Different schema should produce different ID");
    }

    #[test]
    fn derived_view_new_validates_schema() {
        let creator = [0x42u8; 32];

        // Valid: correct data length for schema
        let valid_data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // AggregateVolume
            1000,
            creator,
            valid_data,
        );
        assert!(view.is_some(), "Valid data should create view");

        // Invalid: wrong data length for schema
        let invalid_data = vec![0u8; 10]; // Wrong length
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // AggregateVolume expects 32 bytes
            1000,
            creator,
            invalid_data,
        );
        assert!(view.is_none(), "Invalid data should fail");

        // Invalid: unknown schema ID
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            99, // Unknown schema
            1000,
            creator,
            vec![0u8; 32],
        );
        assert!(view.is_none(), "Unknown schema should fail");
    }

    #[test]
    fn derived_view_encode_decode_roundtrip() {
        let creator = [0x42u8; 32];
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        let view = DerivedView::new(
            DerivedSourceType::ProtocolGenerated,
            1, // AggregateVolume
            5000,
            creator,
            data,
        )
        .unwrap();

        let encoded = encode_derived_view_v1(&view);
        let decoded = decode_derived_view_v1(&encoded).unwrap();

        assert_eq!(view.view_id, decoded.view_id);
        assert_eq!(view.source_type, decoded.source_type);
        assert_eq!(view.schema_id, decoded.schema_id);
        assert_eq!(view.created_at, decoded.created_at);
        assert_eq!(view.creator, decoded.creator);
        assert_eq!(view.data, decoded.data);
    }

    #[test]
    fn derived_view_decode_bad_version() {
        let mut bytes = vec![0u8; 100];
        bytes[0] = 99; // Invalid version

        let result = decode_derived_view_v1(&bytes);
        assert!(matches!(
            result,
            Err(DerivedViewDecodeError::BadVersion {
                expected: 1,
                got: 99
            })
        ));
    }

    #[test]
    fn derived_view_decode_bad_source_type() {
        let creator = [0x42u8; 32];
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        let view =
            DerivedView::new(DerivedSourceType::ChainAggregate, 1, 1000, creator, data).unwrap();

        let mut encoded = encode_derived_view_v1(&view);
        encoded[33] = 99; // Invalid source type at position 33

        let result = decode_derived_view_v1(&encoded);
        assert!(matches!(
            result,
            Err(DerivedViewDecodeError::InvalidSourceType { byte: 99 })
        ));
    }

    #[test]
    fn derived_view_decode_bad_schema() {
        let creator = [0x42u8; 32];
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        let view =
            DerivedView::new(DerivedSourceType::ChainAggregate, 1, 1000, creator, data).unwrap();

        let mut encoded = encode_derived_view_v1(&view);
        // Schema ID is at position 34-37 (after version, view_id, source_type)
        encoded[34..38].copy_from_slice(&99u32.to_be_bytes());

        let result = decode_derived_view_v1(&encoded);
        assert!(matches!(
            result,
            Err(DerivedViewDecodeError::InvalidSchemaId { id: 99 })
        ));
    }

    #[test]
    fn derived_view_decode_too_short() {
        let bytes = vec![1u8; 50]; // Too short for header

        let result = decode_derived_view_v1(&bytes);
        assert!(matches!(
            result,
            Err(DerivedViewDecodeError::UnexpectedEof { .. })
        ));
    }

    // ========================================================================
    // SCHEMA-SPECIFIC DATA TESTS
    // ========================================================================

    #[test]
    fn aggregate_volume_data_roundtrip() {
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 999_999_999_999,
        };

        let encoded = data.encode();
        assert_eq!(encoded.len(), 32);

        let decoded = AggregateVolumeData::decode(&encoded).unwrap();
        assert_eq!(data.start_height, decoded.start_height);
        assert_eq!(data.end_height, decoded.end_height);
        assert_eq!(data.total_volume, decoded.total_volume);
    }

    #[test]
    fn activity_count_data_roundtrip() {
        let data = ActivityCountData {
            start_height: 1000,
            end_height: 2000,
            tx_count: 50000,
        };

        let encoded = data.encode();
        assert_eq!(encoded.len(), 24);

        let decoded = ActivityCountData::decode(&encoded).unwrap();
        assert_eq!(data.start_height, decoded.start_height);
        assert_eq!(data.end_height, decoded.end_height);
        assert_eq!(data.tx_count, decoded.tx_count);
    }

    #[test]
    fn pool_size_data_roundtrip() {
        let data = PoolSizeData {
            snapshot_height: 5000,
            pool_size: 1_000_000_000_000_000,
        };

        let encoded = data.encode();
        assert_eq!(encoded.len(), 24);

        let decoded = PoolSizeData::decode(&encoded).unwrap();
        assert_eq!(data.snapshot_height, decoded.snapshot_height);
        assert_eq!(data.pool_size, decoded.pool_size);
    }

    #[test]
    fn schema_data_decode_too_short() {
        assert!(AggregateVolumeData::decode(&[0u8; 10]).is_none());
        assert!(ActivityCountData::decode(&[0u8; 10]).is_none());
        assert!(PoolSizeData::decode(&[0u8; 10]).is_none());
    }

    // ========================================================================
    // PRIVACY BUDGET TESTS (STUB)
    // ========================================================================

    #[test]
    fn privacy_budget_new() {
        let budget = PrivacyBudget::new();
        assert_eq!(budget.available, MAX_PRIVACY_BUDGET);
        assert_eq!(budget.consumed, 0);
        assert_eq!(budget.last_replenish_height, 0);
    }

    #[test]
    fn privacy_budget_can_read_always_true_in_stub() {
        let budget = PrivacyBudget {
            available: 0, // Even with 0 budget
            consumed: 1000,
            last_replenish_height: 0,
        };
        // In Week 23 stub, always returns true
        assert!(budget.can_read());
    }

    #[test]
    fn privacy_budget_consume_records() {
        let mut budget = PrivacyBudget::new();
        let initial_available = budget.available;

        budget.consume(1);

        assert_eq!(budget.consumed, PRIVACY_BUDGET_PER_VIEW);
        assert_eq!(
            budget.available,
            initial_available - PRIVACY_BUDGET_PER_VIEW
        );
    }

    #[test]
    fn privacy_budget_encode_decode_roundtrip() {
        let budget = PrivacyBudget {
            available: 500,
            consumed: 100,
            last_replenish_height: 10000,
        };

        let encoded = budget.encode();
        assert_eq!(encoded.len(), 24);

        let decoded = PrivacyBudget::decode(&encoded).unwrap();
        assert_eq!(budget.available, decoded.available);
        assert_eq!(budget.consumed, decoded.consumed);
        assert_eq!(budget.last_replenish_height, decoded.last_replenish_height);
    }

    #[test]
    fn privacy_budget_decode_too_short() {
        assert!(PrivacyBudget::decode(&[0u8; 10]).is_none());
    }

    // ========================================================================
    // ENCODING DETERMINISM TESTS
    // ========================================================================

    #[test]
    fn encoding_is_deterministic() {
        let creator = [0x42u8; 32];
        let data = ActivityCountData {
            start_height: 100,
            end_height: 200,
            tx_count: 5000,
        }
        .encode();

        let view =
            DerivedView::new(DerivedSourceType::UserAuthorized, 2, 1000, creator, data).unwrap();

        let enc1 = encode_derived_view_v1(&view);
        let enc2 = encode_derived_view_v1(&view);

        assert_eq!(enc1, enc2, "Encoding must be deterministic");
    }

    #[test]
    fn all_schemas_create_valid_views() {
        let creator = [0x42u8; 32];

        // Schema 1: AggregateVolume
        let data1 = AggregateVolumeData {
            start_height: 0,
            end_height: 100,
            total_volume: 0,
        }
        .encode();
        assert!(
            DerivedView::new(DerivedSourceType::ChainAggregate, 1, 0, creator, data1).is_some()
        );

        // Schema 2: ActivityCount
        let data2 = ActivityCountData {
            start_height: 0,
            end_height: 100,
            tx_count: 0,
        }
        .encode();
        assert!(
            DerivedView::new(DerivedSourceType::ChainAggregate, 2, 0, creator, data2).is_some()
        );

        // Schema 3: PoolSize
        let data3 = PoolSizeData {
            snapshot_height: 0,
            pool_size: 0,
        }
        .encode();
        assert!(
            DerivedView::new(DerivedSourceType::ChainAggregate, 3, 0, creator, data3).is_some()
        );
    }
}
