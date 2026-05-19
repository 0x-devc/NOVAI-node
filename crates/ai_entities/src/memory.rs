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

/// Maximum number of `DelegationGrant` memory objects a single delegator may
/// hold open at one time. Counts against the per-entity object cap as well.
pub const MAX_DELEGATION_GRANTS: u32 = 20;

/// Codec version for `DelegationGrantData`.
pub const DELEGATION_GRANT_VERSION: u8 = 1;

/// Canonical wire size of a `DelegationGrantData` payload (42 bytes).
pub const DELEGATION_GRANT_SIZE: usize = 1 + 32 + 1 + 8;

/// Maximum number of `Subscription` memory objects a single subscriber may
/// hold (active or cancelled) at one time. Counts against the per-entity
/// object cap as well; cancelled records remain in state for audit until
/// the subscriber explicitly deletes them via `DELETE_MEMORY_OBJECT`.
pub const MAX_SUBSCRIPTIONS_PER_ENTITY: u32 = 10;

/// Canonical wire size of a `SubscriptionData` payload (114 bytes).
///
/// Layout: `subscriber_entity_id:32 | producer_entity_id:32 |
/// covered_signal_type:1 | rate_per_block_be:8 | start_height_be:8 |
/// end_height_be:8 | last_settled_height_be:8 | total_locked_be:16 |
/// is_active:1`.
pub const SUBSCRIPTION_SIZE: usize = 32 + 32 + 1 + 8 + 8 + 8 + 8 + 16 + 1;

/// Maximum number of `ServiceDescriptor` memory objects a single entity
/// may publish (Week 29).
///
/// Counts against the global `MAX_MEMORY_OBJECTS_PER_ENTITY` cap. 16 was
/// chosen to leave room for the other memory types an active entity
/// typically holds (subscriptions, delegation grants, ratings, etc.)
/// while still allowing a publisher to advertise a meaningful catalogue
/// of services per category.
pub const MAX_SERVICE_DESCRIPTORS_PER_ENTITY: u32 = 16;

/// Wire-format version byte carried at offset 0 of every
/// `ServiceDescriptorData` payload.
pub const SERVICE_DESCRIPTOR_V1: u8 = 1;

/// Canonical wire size of a `ServiceDescriptorData` payload (144 bytes).
///
/// Layout: `version:1 | service_name_hash:32 | service_url_hash:32 |
/// description_hash:32 | category:1 | price_per_call_be:8 |
/// subscription_rate_per_block_be:8 | min_reputation_score_be:2 |
/// min_stake_be:16 | capability_tags_be:4 | status:1 | reserved:7`.
pub const SERVICE_DESCRIPTOR_SIZE: usize = 1 + 32 + 32 + 32 + 1 + 8 + 8 + 2 + 16 + 4 + 1 + 7;

/// `ServiceDescriptor` category discriminants. Values 0..=15 are
/// well-known v1 categories; 16..=255 are reserved for governance
/// allocation. The handler rejects creates whose `category` byte is
/// above `SERVICE_CATEGORY_RESERVED_MAX`.
pub const SERVICE_CATEGORY_GENERIC: u8 = 0;
/// Real-world data feeds (e.g., prices, weather, sports scores).
pub const SERVICE_CATEGORY_DATA_ORACLE: u8 = 1;
/// LLM or ML model inference as a service.
pub const SERVICE_CATEGORY_INFERENCE: u8 = 2;
/// Computation as a service (e.g., proof generation, batch jobs).
pub const SERVICE_CATEGORY_COMPUTE: u8 = 3;
/// Off-chain data persistence (e.g., IPFS pinning, archival).
pub const SERVICE_CATEGORY_STORAGE: u8 = 4;
/// Chain or data indexing (e.g., GraphQL endpoints, search indexes).
pub const SERVICE_CATEGORY_INDEXER: u8 = 5;
/// On-chain signal producer (anomaly, prediction, congestion forecasting).
pub const SERVICE_CATEGORY_SIGNAL_PROVIDER: u8 = 6;
/// Proof verification or audit services.
pub const SERVICE_CATEGORY_VERIFICATION: u8 = 7;
/// Monitoring / observability services (uptime checks, log analysis,
/// metric scraping).
pub const SERVICE_CATEGORY_MONITORING: u8 = 8;
/// Gateway / proxy services that route requests to other endpoints
/// (e.g., HTTPS-to-x402 bridges, multi-chain RPC proxies).
pub const SERVICE_CATEGORY_GATEWAY: u8 = 9;
/// Maximum well-known service category discriminant. Values up to and
/// including this are accepted at create time without governance
/// approval. Values 16..=255 are reserved for future governance-
/// allocated categories; bumping this constant is the gate.
pub const SERVICE_CATEGORY_RESERVED_MAX: u8 = 15;

/// `ServiceDescriptor` status discriminants. The handler rejects
/// creates and updates whose `status` byte is above
/// `SERVICE_STATUS_MAX`.
pub const SERVICE_STATUS_ACTIVE: u8 = 0;
/// Publisher has temporarily suspended the service. Discovery RPC
/// surfaces it; clients SHOULD treat it as unavailable.
pub const SERVICE_STATUS_PAUSED: u8 = 1;
/// Service is no longer offered. Publisher SHOULD delete the
/// descriptor; status is retained for callers tracking historical
/// references.
pub const SERVICE_STATUS_DEPRECATED: u8 = 2;
/// Maximum valid `ServiceDescriptor` status discriminant.
pub const SERVICE_STATUS_MAX: u8 = SERVICE_STATUS_DEPRECATED;

/// Maximum number of `VkRegistration` memory objects a single entity may
/// publish (Week 30).
///
/// Counts against the global `MAX_MEMORY_OBJECTS_PER_ENTITY` cap. The cap
/// is intentionally tight: verification keys are bulky (hundreds of bytes
/// to several KB compressed for BN254 Groth16), and in v1 most entities
/// will register one VK per circuit they own. The pattern mirrors the
/// per-entity caps already in place for `DelegationGrant` and
/// `ServiceDescriptor`.
pub const MAX_VK_REGISTRATIONS_PER_ENTITY: u32 = 8;

/// Wire-format version byte carried at offset 0 of every
/// `VkRegistrationData` payload (Week 30).
pub const VK_REGISTRATION_VERSION: u8 = 1;

/// Maximum length of the optional `label` field carried by a
/// `VkRegistrationData` payload, in bytes. The label is free-form UTF-8
/// metadata for human-readable identification of registered VKs
/// (e.g., `"sum-circuit-v1"`). Capping at 32 bytes keeps the per-object
/// storage footprint bounded while leaving enough room for a meaningful
/// tag.
pub const VK_REGISTRATION_LABEL_MAX: usize = 32;

/// Fixed-size header of a `VkRegistrationData` payload, in bytes.
///
/// Layout: `version:1 | proof_type:1 | code_hash:32 | label_len:1 |
/// vk_len_be:4`. Total = 39 bytes. The full payload appends `label_len`
/// UTF-8 label bytes followed by `vk_len` compressed VK bytes.
pub const VK_REGISTRATION_HEADER_SIZE: usize = 1 + 1 + 32 + 1 + 4;

/// Maximum number of `SlaAgreement` memory objects a single buyer may
/// open (Week 31). Counted against the global
/// `MAX_MEMORY_OBJECTS_PER_ENTITY` cap and against this per-type cap by
/// the create handler via a bounded prefix scan of
/// `ai/memory_by_type/14/<buyer>/`.
///
/// The cap is per BUYER (memory-object owner). Sellers are not capped
/// in v1: they appear in arbitrarily many SLAs but never own the
/// underlying memory object. Cap value mirrors
/// `MAX_VK_REGISTRATIONS_PER_ENTITY` (a deliberately tight v1 cap that
/// can be raised by future governance once usage patterns are known).
pub const MAX_SLAS_PER_ENTITY: u32 = 8;

/// Wire-format version byte carried at offset 0 of every
/// `SlaAgreementData` payload.
pub const SLA_AGREEMENT_V1: u8 = 1;

/// Canonical wire size of a `SlaAgreementData` payload, in bytes.
///
/// Layout: `version:1 | buyer_entity_id:32 | seller_entity_id:32 |
/// service_descriptor_hash:32 | status:1 | created_at_height_be:8 |
/// accepted_at_height_be:8 | start_height_be:8 | end_height_be:8 |
/// violation_count_be:4 | violation_threshold_be:4 |
/// max_response_time_blocks_be:4 | min_uptime_bps_be:2 |
/// min_delivery_success_bps_be:2 | price_per_call_be:8 |
/// slash_amount_be:16 | terminated_at_height_be:8 | slashed_amount_be:16 |
/// reserved:16`. Total = 210 bytes.
pub const SLA_AGREEMENT_SIZE: usize =
    1 + 32 + 32 + 32 + 1 + 8 + 8 + 8 + 8 + 4 + 4 + 4 + 2 + 2 + 8 + 16 + 8 + 16 + 16;

/// `SlaAgreement` lifecycle status discriminants. Mutations between
/// statuses are runtime-controlled by the create / `SlaAccept` /
/// `ServiceAttestation` (auto-slash) / `DeleteMemoryObject` handlers;
/// the v1 wire format reserves `SLA_STATUS_COMPLETED` for a future
/// explicit-finalize signal.
pub const SLA_STATUS_PROPOSED: u8 = 0;
/// Seller has accepted the SLA; violations counted while
/// `current_height` is in `[start_height, end_height]`.
pub const SLA_STATUS_ACTIVE: u8 = 1;
/// RESERVED in v1: no on-chain transition writes this byte. Expired
/// SLAs stay in `SLA_STATUS_ACTIVE` and surface `is_expired = true`
/// at the RPC layer. Defined here so a future `SlaFinalize` signal
/// can land without a schema bump.
pub const SLA_STATUS_COMPLETED: u8 = 2;
/// Auto-slash fired (violation count reached threshold); seller's
/// stake was debited and the active-between index entry removed. The
/// SLA persists in this terminal state for audit until the buyer
/// deletes the memory object.
pub const SLA_STATUS_VIOLATED: u8 = 3;
/// Buyer deleted a `SLA_STATUS_PROPOSED` SLA before acceptance.
/// RESERVED in v1: the byte is never written to storage (cancellation
/// just deletes the memory object). Defined for the RPC label
/// surface in case a future variant retains a deleted-but-not-purged
/// record.
pub const SLA_STATUS_CANCELLED: u8 = 4;
/// Maximum valid `SlaAgreement` status discriminant.
pub const SLA_STATUS_MAX: u8 = SLA_STATUS_CANCELLED;

/// Number of trailing reserved bytes carried by every
/// `SlaAgreementData` payload. MUST be zero on create / update; decode
/// preserves them so future field allocations are forward-compatible
/// with the v1 binary layout.
pub const SLA_RESERVED_LEN: usize = 16;

/// Maximum allowed value of the `min_uptime_bps` field (10000 bps =
/// 100%). The runtime does NOT enforce uptime in v1; the field is
/// validated for forward-compat range correctness only.
pub const SLA_MIN_UPTIME_BPS_MAX: u16 = 10_000;

/// Maximum allowed value of the `min_delivery_success_bps` field
/// (10000 bps = 100%). The runtime does NOT enforce delivery success
/// rate in v1; the field is validated for range correctness only.
pub const SLA_MIN_DELIVERY_SUCCESS_BPS_MAX: u16 = 10_000;

/// Maximum span in blocks between `start_height` and `end_height` for
/// a newly created `SlaAgreement` (`end_height - start_height <=
/// SLA_MAX_DURATION_BLOCKS`). Default = 604 800 blocks (~1 week at
/// 1 block/s). Bounds the time a memory-object slot can remain locked
/// to a single SLA and limits the worst-case scan cost of the lazy
/// `StakeWithdraw` collateral check shipped in Phase 4.
pub const SLA_MAX_DURATION_BLOCKS: u64 = 604_800;

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
/// - `CompositionGraph`: Per-entity declaration of inbound signal
///   dependencies; payload is the canonical `CompositionGraphData` encoding
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
    /// Cross-entity composition graph: a list of source-entity dependencies
    /// the owning entity declares it consumes signals from. Used by oracle
    /// `CompositionCheck` signals to verify dependency health and auto-pause
    /// the owner when a required dependency has failed.
    CompositionGraph = 8,
    /// On-chain audit record of a verified ZK proof submission. Created by
    /// the `ProofSubmission` signal handler when the verifier accepts a
    /// proof. Fixed 105-byte payload: proof_type:1 | code_hash:32 |
    /// computation_hash:32 | proof_hash:32 | height_be:8.
    VerificationRecord = 9,
    /// On-chain delegation grant: an AI entity (the memory object owner /
    /// delegator) authorizes another AI entity (the delegate, identified by
    /// the embedded `delegate_entity_id`) to act with a subset of the
    /// delegator's capabilities for a bounded duration. Stored as a fixed
    /// 42-byte payload (`DelegationGrantData`). Revocation is performed by
    /// deleting the memory object via `DELETE_MEMORY_OBJECT`.
    DelegationGrant = 10,
    /// Recurring payment subscription record (Feature 9): the memory
    /// object owner is the SUBSCRIBER, who has locked
    /// `rate_per_block * duration_blocks` of `economic_balance` to a
    /// producer for a fixed covered signal type. Settlement is lazy and
    /// triggered by the subscriber's `SubscriptionCancel` signal; on
    /// cancel, accrued payment is transferred to the producer (less the
    /// 2% marketplace fee), the 5% cancel fee on remaining locked funds
    /// is paid to the producer, the rest is refunded to the subscriber,
    /// and `is_active` is set to false. Stored as a fixed 114-byte
    /// payload (`SubscriptionData`).
    Subscription = 11,
    /// Agent Discovery Registry entry (Week 29): the memory object owner
    /// is publishing a service description that other entities can
    /// discover on-chain. The data field carries a fixed 144-byte
    /// `ServiceDescriptorData` describing what the service is
    /// (category + off-chain name / URL / description hashes), what it
    /// charges (per-call price, optional subscription rate), and what
    /// callers must satisfy (minimum reputation score, minimum stake).
    /// `category` drives a dedicated `by_category` discovery index;
    /// updates may change any field except the category (a category
    /// change requires delete + recreate). The object's `object_id` is
    /// the canonical handle a payer puts in the
    /// `service_descriptor_hash` field of a Week 28 `PaymentRequest`.
    ServiceDescriptor = 12,
    /// VK Registry entry (Week 30): the memory object owner publishes
    /// a zero-knowledge proof verification key on-chain so that future
    /// `ProofSubmission` signals carrying
    /// `PROOF_TYPE_GROTH16_REGISTERED` can reference the VK by the
    /// containing memory object's 32-byte `object_id` instead of
    /// inlining the full ~500-byte VK every time. The data field
    /// carries a `VkRegistrationData` payload binding the VK to a
    /// `code_hash` (the computation it verifies) and a proof_type
    /// discriminant. The proof_type, code_hash, and vk_bytes fields
    /// are IMMUTABLE on `UpdateMemoryObject`; only the free-form
    /// `label` tag is mutable.
    VkRegistration = 13,
    /// On-chain Service Level Agreement between two AI entities
    /// (Week 31). The memory object owner is the BUYER who proposed
    /// the agreement; the embedded `seller_entity_id` is the
    /// counterparty that accepts (via the `SlaAccept` signal) and
    /// whose stake is at risk on threshold breach. Violations are
    /// counted from `PAYMENT_ATTESTATION_STATUS_FAILED`
    /// `ServiceAttestation` signals issued by the buyer while
    /// `current_height` is in `[start_height, end_height]`; when
    /// `violation_count >= violation_threshold` the runtime auto-
    /// slashes `slash_amount` from the seller's `stake_balance`
    /// (saturating, mirroring `StakeSlash`), credits
    /// `KEY_SLASH_TREASURY`, applies an additional
    /// `REP_DELTA_SLA_VIOLATION_TRIGGERED` to the seller, and
    /// transitions the SLA to `SLA_STATUS_VIOLATED`. The agreement
    /// is binding once accepted: no mutual cancel signal in v1.
    SlaAgreement = 14,
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
            8 => Some(Self::CompositionGraph),
            9 => Some(Self::VerificationRecord),
            10 => Some(Self::DelegationGrant),
            11 => Some(Self::Subscription),
            12 => Some(Self::ServiceDescriptor),
            13 => Some(Self::VkRegistration),
            14 => Some(Self::SlaAgreement),
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
            Self::CompositionGraph => "CompositionGraph",
            Self::VerificationRecord => "VerificationRecord",
            Self::DelegationGrant => "DelegationGrant",
            Self::Subscription => "Subscription",
            Self::ServiceDescriptor => "ServiceDescriptor",
            Self::VkRegistration => "VkRegistration",
            Self::SlaAgreement => "SlaAgreement",
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
// COMPOSITION GRAPH (cross-entity dependency declarations)
// ============================================================================

/// Maximum number of inbound dependencies a single entity may declare.
pub const MAX_COMPOSITION_DEPENDENCIES: usize = 10;

/// On-wire size of one `CompositionDependency`
/// (source_entity_id + required_signal_type + min_reputation_be +
/// min_stake_be + is_required).
pub const COMPOSITION_DEPENDENCY_SIZE: usize = 32 + 1 + 2 + 8 + 1;

/// Maximum encoded size of a full `CompositionGraphData` payload
/// (`1` count byte + up to `MAX_COMPOSITION_DEPENDENCIES`
/// × `COMPOSITION_DEPENDENCY_SIZE`).
pub const COMPOSITION_GRAPH_MAX_SIZE: usize =
    1 + MAX_COMPOSITION_DEPENDENCIES * COMPOSITION_DEPENDENCY_SIZE;

/// Fixed on-wire size of a `VerificationRecordData` payload.
/// `proof_type:1 | code_hash:32 | computation_hash:32 | proof_hash:32 |
/// height_be:8`.
pub const VERIFICATION_RECORD_SIZE: usize = 1 + 32 + 32 + 32 + 8;

/// One inbound dependency within a `CompositionGraph`.
///
/// Layout (44 bytes):
/// `source_entity_id:32 | required_signal_type:1 | min_reputation_be:2 |
/// min_stake_be:8 | is_required:1`.
///
/// `is_required` is encoded as `0` (advisory only) or `1` (auto-pause owner
/// when this dependency fails); decoders reject any other value.
///
/// `min_reputation` is `0` to accept any reputation. `min_stake` is `0`
/// (units) to accept any stake balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositionDependency {
    /// 32-byte ID of the entity this owner depends on.
    pub source_entity_id: [u8; 32],
    /// `AiSignalType` byte the owner consumes from the source.
    pub required_signal_type: u8,
    /// Minimum reputation score the source must hold (0 = any).
    pub min_reputation: u16,
    /// Minimum stake balance the source must hold, in smallest units
    /// (0 = any). u64 caps below the actual u128 stake balance but is
    /// sufficient for any practical threshold.
    pub min_stake: u64,
    /// Whether failure of this dependency auto-pauses the owning entity.
    pub is_required: bool,
}

/// An owning entity's declared cross-entity dependencies, stored as the data
/// field of a `MemoryObjectType::CompositionGraph` memory object.
///
/// On-wire layout: `count:1 | entries: count * COMPOSITION_DEPENDENCY_SIZE`,
/// where `count <= MAX_COMPOSITION_DEPENDENCIES`.
///
/// Unlike `SignalCatalogData`, the codec rejects duplicates: no two entries
/// may share the same `(source_entity_id, required_signal_type)` pair.
/// Composition semantics cannot tolerate ambiguity at a given dependency
/// index, so duplicates are a hard error rather than a warning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompositionGraphData {
    /// Inbound dependencies (max `MAX_COMPOSITION_DEPENDENCIES`).
    pub dependencies: Vec<CompositionDependency>,
}

impl CompositionDependency {
    /// Encode this dependency to its 44-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; COMPOSITION_DEPENDENCY_SIZE] {
        let mut out = [0u8; COMPOSITION_DEPENDENCY_SIZE];
        out[0..32].copy_from_slice(&self.source_entity_id);
        out[32] = self.required_signal_type;
        out[33..35].copy_from_slice(&self.min_reputation.to_be_bytes());
        out[35..43].copy_from_slice(&self.min_stake.to_be_bytes());
        out[43] = u8::from(self.is_required);
        out
    }

    /// Decode a single dependency from a 44-byte slice.
    ///
    /// Returns `None` if the slice is too short or `is_required` is not
    /// `0`/`1`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < COMPOSITION_DEPENDENCY_SIZE {
            return None;
        }
        let mut source_entity_id = [0u8; 32];
        source_entity_id.copy_from_slice(&bytes[0..32]);
        let required_signal_type = bytes[32];
        let min_reputation = u16::from_be_bytes(bytes[33..35].try_into().ok()?);
        let min_stake = u64::from_be_bytes(bytes[35..43].try_into().ok()?);
        let is_required = match bytes[43] {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Self {
            source_entity_id,
            required_signal_type,
            min_reputation,
            min_stake,
            is_required,
        })
    }
}

impl CompositionGraphData {
    /// Encode this graph to canonical bytes.
    ///
    /// Silently truncates to `MAX_COMPOSITION_DEPENDENCIES` if the entries
    /// vec is longer (mirroring `SignalCatalogData::encode`). Owners are
    /// expected to maintain a graph within bounds and rely on the matching
    /// decode-side rejection to catch inconsistencies.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let count = self.dependencies.len().min(MAX_COMPOSITION_DEPENDENCIES);
        let mut out = Vec::with_capacity(1 + count * COMPOSITION_DEPENDENCY_SIZE);
        #[allow(clippy::cast_possible_truncation)]
        out.push(count as u8);
        for dep in self.dependencies.iter().take(count) {
            out.extend_from_slice(&dep.encode());
        }
        out
    }

    /// Decode a graph from canonical bytes.
    ///
    /// Returns `None` if the count byte exceeds `MAX_COMPOSITION_DEPENDENCIES`,
    /// the buffer is shorter than `1 + count * COMPOSITION_DEPENDENCY_SIZE`,
    /// any entry fails to decode, or two entries share the same
    /// `(source_entity_id, required_signal_type)` pair.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let count = bytes[0] as usize;
        if count > MAX_COMPOSITION_DEPENDENCIES {
            return None;
        }
        let expected_len = 1 + count * COMPOSITION_DEPENDENCY_SIZE;
        if bytes.len() < expected_len {
            return None;
        }
        let mut dependencies = Vec::with_capacity(count);
        for i in 0..count {
            let off = 1 + i * COMPOSITION_DEPENDENCY_SIZE;
            let dep =
                CompositionDependency::decode(&bytes[off..off + COMPOSITION_DEPENDENCY_SIZE])?;
            // Codec-level duplicate rejection: no two deps may share the same
            // (source_entity_id, required_signal_type) pair.
            for prev in &dependencies {
                let p: &CompositionDependency = prev;
                if p.source_entity_id == dep.source_entity_id
                    && p.required_signal_type == dep.required_signal_type
                {
                    return None;
                }
            }
            dependencies.push(dep);
        }
        Some(Self { dependencies })
    }
}

/// On-chain audit record of a successful ZK proof verification.
///
/// Stored as the data field of a `MemoryObjectType::VerificationRecord`
/// memory object, owned by the entity that submitted the proof. The
/// `ProofSubmission` signal handler creates one record per accepted
/// proof; the record is immutable for the lifetime of the memory object.
///
/// Wire layout (105 bytes, fixed):
/// `proof_type:1 | code_hash:32 | computation_hash:32 | proof_hash:32 |
/// height_be:8`.
///
/// `proof_hash` is `blake3(proof_bytes)` — the hash of the actual proof
/// material (which is NOT carried inline in the `ProofSubmission` signal
/// payload; future real verifiers will resolve proof bytes via the
/// off-chain artifact referenced by the signal commitment hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationRecordData {
    /// Discriminant identifying the proof system (e.g. stub / Groth16 / PLONK).
    pub proof_type: u8,
    /// Hash of the AI module code/weights the proof attests to.
    pub code_hash: [u8; 32],
    /// Hash of the computation context (inputs/outputs) the proof asserts.
    pub computation_hash: [u8; 32],
    /// `blake3` hash of the proof bytes that were verified.
    pub proof_hash: [u8; 32],
    /// Block height at which the proof was verified and recorded.
    pub height: u64,
}

impl VerificationRecordData {
    /// Encode this record to its 105-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; VERIFICATION_RECORD_SIZE] {
        let mut out = [0u8; VERIFICATION_RECORD_SIZE];
        out[0] = self.proof_type;
        out[1..33].copy_from_slice(&self.code_hash);
        out[33..65].copy_from_slice(&self.computation_hash);
        out[65..97].copy_from_slice(&self.proof_hash);
        out[97..105].copy_from_slice(&self.height.to_be_bytes());
        out
    }

    /// Decode a record from canonical bytes.
    ///
    /// Returns `None` if the slice length is not exactly
    /// `VERIFICATION_RECORD_SIZE`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != VERIFICATION_RECORD_SIZE {
            return None;
        }
        let proof_type = bytes[0];
        let mut code_hash = [0u8; 32];
        code_hash.copy_from_slice(&bytes[1..33]);
        let mut computation_hash = [0u8; 32];
        computation_hash.copy_from_slice(&bytes[33..65]);
        let mut proof_hash = [0u8; 32];
        proof_hash.copy_from_slice(&bytes[65..97]);
        let height = u64::from_be_bytes(bytes[97..105].try_into().ok()?);
        Some(Self {
            proof_type,
            code_hash,
            computation_hash,
            proof_hash,
            height,
        })
    }
}

/// On-chain delegation grant payload data.
///
/// Stored as the data field of a `MemoryObjectType::DelegationGrant`
/// memory object. The owner of the surrounding memory object envelope is
/// the DELEGATOR (Entity A); the embedded `delegate_entity_id` identifies
/// the recipient (Entity B). While the grant is active, its
/// `granted_capabilities` bits are merged into the delegate's effective
/// capabilities at signal/memory-CRUD admission time.
///
/// Wire layout (42 bytes, fixed):
/// `version:1 | delegate_entity_id:32 | granted_capabilities:1 | expires_at_be:8`.
///
/// `expires_at == 0` is the explicit no-expiry sentinel; the grant
/// remains active until the delegator deletes the memory object.
/// Revocation is performed via `DELETE_MEMORY_OBJECT`; updates of a
/// `DelegationGrant` memory object are rejected by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegationGrantData {
    /// Codec version (must equal `DELEGATION_GRANT_VERSION`).
    pub version: u8,
    /// AI entity ID that receives the delegated capabilities.
    pub delegate_entity_id: [u8; 32],
    /// Capability bits granted to the delegate (same byte layout as
    /// `Capabilities::to_byte`).
    pub granted_capabilities: u8,
    /// Block height at which the grant expires; `0` means no expiry.
    pub expires_at: u64,
}

impl DelegationGrantData {
    /// Encode this grant to its 42-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; DELEGATION_GRANT_SIZE] {
        let mut out = [0u8; DELEGATION_GRANT_SIZE];
        out[0] = self.version;
        out[1..33].copy_from_slice(&self.delegate_entity_id);
        out[33] = self.granted_capabilities;
        out[34..42].copy_from_slice(&self.expires_at.to_be_bytes());
        out
    }

    /// Decode a grant from canonical bytes.
    ///
    /// Returns `None` if the slice length is not exactly
    /// `DELEGATION_GRANT_SIZE` or the version byte does not match
    /// `DELEGATION_GRANT_VERSION`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != DELEGATION_GRANT_SIZE {
            return None;
        }
        let version = bytes[0];
        if version != DELEGATION_GRANT_VERSION {
            return None;
        }
        let mut delegate_entity_id = [0u8; 32];
        delegate_entity_id.copy_from_slice(&bytes[1..33]);
        let granted_capabilities = bytes[33];
        let expires_at = u64::from_be_bytes(bytes[34..42].try_into().ok()?);
        Some(Self {
            version,
            delegate_entity_id,
            granted_capabilities,
            expires_at,
        })
    }

    /// True when the grant is currently usable at the given block height.
    ///
    /// `expires_at == 0` is treated as no expiry. Otherwise the grant is
    /// active strictly while `current_height < expires_at`.
    #[must_use]
    pub const fn is_active_at(&self, current_height: u64) -> bool {
        self.expires_at == 0 || current_height < self.expires_at
    }
}

/// On-chain subscription record payload data (Feature 9).
///
/// Stored as the data field of a `MemoryObjectType::Subscription` memory
/// object. The owner of the surrounding memory object envelope is the
/// SUBSCRIBER, who has locked `rate_per_block * (end_height - start_height)`
/// units of `economic_balance` at creation time. The `producer_entity_id`
/// embedded here identifies the counterparty that will receive accrued
/// payment when settlement occurs.
///
/// Wire layout (114 bytes, fixed):
/// `subscriber_entity_id:32 | producer_entity_id:32 |
/// covered_signal_type:1 | rate_per_block_be:8 | start_height_be:8 |
/// end_height_be:8 | last_settled_height_be:8 | total_locked_be:16 |
/// is_active:1`.
///
/// Settlement is performed lazily by the `SubscriptionCancel` signal
/// handler. The handler:
///   1. Computes accrued blocks as
///      `min(current_height, end_height) - last_settled_height`.
///   2. Credits the producer with `accrued_blocks * rate_per_block` less
///      the standard 2% marketplace fee (which accumulates in the
///      marketplace treasury).
///   3. Computes the unaccrued remainder as
///      `total_locked - accrued_blocks * rate_per_block`.
///   4. Pays the producer a 5% cancel fee on the remainder (no
///      marketplace cut on this fee, by design).
///   5. Refunds the rest of the remainder to the subscriber.
///   6. Sets `is_active = false` and advances `last_settled_height` to
///      `min(current_height, end_height)`.
///
/// Cancelled records remain in state for audit. The subscriber may
/// reclaim the memory object slot by issuing a `DELETE_MEMORY_OBJECT`
/// transaction. Only the original subscriber may cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionData {
    /// AI entity that pays for and owns this subscription.
    pub subscriber_entity_id: [u8; 32],
    /// AI entity that the subscriber is paying.
    pub producer_entity_id: [u8; 32],
    /// `AiSignalType` byte identifying which producer signal type the
    /// subscription covers (informational; not enforced by the runtime).
    pub covered_signal_type: u8,
    /// Per-block payment rate, in base units of `economic_balance`.
    pub rate_per_block: u64,
    /// Block height at which the subscription began.
    pub start_height: u64,
    /// Block height at which the subscription naturally expires (the
    /// upper bound for accrual; cancellations after this height accrue
    /// no extra blocks).
    pub end_height: u64,
    /// Block height up to which payment has already been settled to the
    /// producer. Initialized to `start_height` at creation.
    pub last_settled_height: u64,
    /// Total amount locked at creation:
    /// `rate_per_block * (end_height - start_height)`.
    pub total_locked: u128,
    /// `false` after the subscription has been cancelled or fully
    /// settled. Only `true` records authorize further settlement.
    pub is_active: bool,
}

impl SubscriptionData {
    /// Encode this subscription record to its 114-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; SUBSCRIPTION_SIZE] {
        let mut out = [0u8; SUBSCRIPTION_SIZE];
        out[0..32].copy_from_slice(&self.subscriber_entity_id);
        out[32..64].copy_from_slice(&self.producer_entity_id);
        out[64] = self.covered_signal_type;
        out[65..73].copy_from_slice(&self.rate_per_block.to_be_bytes());
        out[73..81].copy_from_slice(&self.start_height.to_be_bytes());
        out[81..89].copy_from_slice(&self.end_height.to_be_bytes());
        out[89..97].copy_from_slice(&self.last_settled_height.to_be_bytes());
        out[97..113].copy_from_slice(&self.total_locked.to_be_bytes());
        out[113] = u8::from(self.is_active);
        out
    }

    /// Decode a subscription record from canonical bytes.
    ///
    /// Returns `None` if the slice length is not exactly
    /// `SUBSCRIPTION_SIZE` or the `is_active` byte is not `0` or `1`.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SUBSCRIPTION_SIZE {
            return None;
        }
        let mut subscriber_entity_id = [0u8; 32];
        subscriber_entity_id.copy_from_slice(&bytes[0..32]);
        let mut producer_entity_id = [0u8; 32];
        producer_entity_id.copy_from_slice(&bytes[32..64]);
        let covered_signal_type = bytes[64];
        let rate_per_block = u64::from_be_bytes(bytes[65..73].try_into().ok()?);
        let start_height = u64::from_be_bytes(bytes[73..81].try_into().ok()?);
        let end_height = u64::from_be_bytes(bytes[81..89].try_into().ok()?);
        let last_settled_height = u64::from_be_bytes(bytes[89..97].try_into().ok()?);
        let total_locked = u128::from_be_bytes(bytes[97..113].try_into().ok()?);
        let is_active = match bytes[113] {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Self {
            subscriber_entity_id,
            producer_entity_id,
            covered_signal_type,
            rate_per_block,
            start_height,
            end_height,
            last_settled_height,
            total_locked,
            is_active,
        })
    }

    /// Compute the number of blocks eligible for settlement at the given
    /// height. Saturates at `end_height`; returns 0 if the record is
    /// already inactive or `current_height <= last_settled_height`.
    #[must_use]
    pub fn settlable_blocks(&self, current_height: u64) -> u64 {
        if !self.is_active {
            return 0;
        }
        let cap = if current_height < self.end_height {
            current_height
        } else {
            self.end_height
        };
        cap.saturating_sub(self.last_settled_height)
    }

    /// Compute the gross accrued payment at the given height
    /// (`settlable_blocks * rate_per_block`). Returns `None` on overflow.
    #[must_use]
    pub fn accrued_gross(&self, current_height: u64) -> Option<u128> {
        let blocks = u128::from(self.settlable_blocks(current_height));
        let rate = u128::from(self.rate_per_block);
        blocks.checked_mul(rate)
    }
}

// ============================================================================
// Agent Discovery Registry payload (Week 29 - D29.2)
// ============================================================================

/// On-chain service descriptor carried in a `MemoryObjectType::ServiceDescriptor`
/// memory object.
///
/// Publishers (AI entities offering a service) put one descriptor per
/// service they offer. Discoverers query the chain by category to find
/// services, then read off-chain documents committed to by the three
/// 32-byte hash fields. The `object_id` of the containing memory object
/// is the canonical handle to put in the `service_descriptor_hash`
/// field of a Week 28 `PaymentRequest`.
///
/// The struct is fixed-size (`SERVICE_DESCRIPTOR_SIZE` = 144 bytes) and
/// carries no free-form text on chain; descriptive content lives off-
/// chain and is committed to via `service_name_hash`,
/// `service_url_hash`, and `description_hash`. The `reserved` field is
/// preserved verbatim by encode and decode; the runtime rejects creates
/// whose `reserved` bytes are non-zero so a future field allocation can
/// add semantics without ambiguity over old descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceDescriptorData {
    /// Wire-format version byte. Must equal `SERVICE_DESCRIPTOR_V1`.
    pub version: u8,
    /// `blake3` commitment to the off-chain canonical service name.
    pub service_name_hash: [u8; 32],
    /// `blake3` commitment to the off-chain canonical endpoint URL.
    /// Clients resolve this to the actual HTTPS / on-chain target
    /// address after reading the descriptor.
    pub service_url_hash: [u8; 32],
    /// `blake3` commitment to the off-chain long description (e.g.,
    /// SLA, supported parameters, pricing schedule).
    pub description_hash: [u8; 32],
    /// Service category discriminant. Must be `<= SERVICE_CATEGORY_RESERVED_MAX`
    /// at create time; values above the well-known range are reserved
    /// for future governance allocation. The category is IMMUTABLE
    /// after publish: updates that change it are rejected so the
    /// `ai/service_descriptors/by_category/` discovery index does not
    /// need to be rewritten on update.
    pub category: u8,
    /// Per-call price in base units of `economic_balance`. `0` means
    /// the service is free.
    pub price_per_call: u64,
    /// Per-block subscription rate in base units of `economic_balance`.
    /// `0` means no subscription pricing is offered for this service.
    pub subscription_rate_per_block: u64,
    /// Minimum reputation score the publisher requires from callers.
    /// Clamped at creation/update to `[0, MAX_REPUTATION_SCORE]` (=100).
    pub min_reputation_score: u16,
    /// Minimum `stake_balance` the publisher requires from callers.
    pub min_stake: u128,
    /// Bitfield of cross-cutting service attributes (low-latency,
    /// deterministic, requires-payment, etc.). No on-chain validation
    /// of bit combinations; this is metadata for client filters.
    pub capability_tags: u32,
    /// Lifecycle status: `SERVICE_STATUS_ACTIVE` (0), `_PAUSED` (1),
    /// or `_DEPRECATED` (2). Status is mutable via update.
    pub status: u8,
    /// 7 bytes reserved for future fields. MUST be zero at create/update
    /// time; decode preserves them so a future field allocation is
    /// forward-compatible with the v1 binary layout.
    pub reserved: [u8; 7],
}

impl ServiceDescriptorData {
    /// Encode this descriptor to its 144-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; SERVICE_DESCRIPTOR_SIZE] {
        let mut out = [0u8; SERVICE_DESCRIPTOR_SIZE];
        out[0] = self.version;
        out[1..33].copy_from_slice(&self.service_name_hash);
        out[33..65].copy_from_slice(&self.service_url_hash);
        out[65..97].copy_from_slice(&self.description_hash);
        out[97] = self.category;
        out[98..106].copy_from_slice(&self.price_per_call.to_be_bytes());
        out[106..114].copy_from_slice(&self.subscription_rate_per_block.to_be_bytes());
        out[114..116].copy_from_slice(&self.min_reputation_score.to_be_bytes());
        out[116..132].copy_from_slice(&self.min_stake.to_be_bytes());
        out[132..136].copy_from_slice(&self.capability_tags.to_be_bytes());
        out[136] = self.status;
        out[137..144].copy_from_slice(&self.reserved);
        out
    }

    /// Decode a service descriptor from canonical bytes.
    ///
    /// Returns `None` if the slice length is not exactly
    /// `SERVICE_DESCRIPTOR_SIZE`. Field-level validation (version,
    /// category range, status range, reputation cap, zero-reserved) is
    /// the handler's responsibility - decode preserves byte content
    /// verbatim so the runtime can produce specific error variants for
    /// each invalid field.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SERVICE_DESCRIPTOR_SIZE {
            return None;
        }
        let mut service_name_hash = [0u8; 32];
        service_name_hash.copy_from_slice(&bytes[1..33]);
        let mut service_url_hash = [0u8; 32];
        service_url_hash.copy_from_slice(&bytes[33..65]);
        let mut description_hash = [0u8; 32];
        description_hash.copy_from_slice(&bytes[65..97]);
        let price_per_call = u64::from_be_bytes(bytes[98..106].try_into().ok()?);
        let subscription_rate_per_block = u64::from_be_bytes(bytes[106..114].try_into().ok()?);
        let min_reputation_score = u16::from_be_bytes(bytes[114..116].try_into().ok()?);
        let min_stake = u128::from_be_bytes(bytes[116..132].try_into().ok()?);
        let capability_tags = u32::from_be_bytes(bytes[132..136].try_into().ok()?);
        let mut reserved = [0u8; 7];
        reserved.copy_from_slice(&bytes[137..144]);
        Some(Self {
            version: bytes[0],
            service_name_hash,
            service_url_hash,
            description_hash,
            category: bytes[97],
            price_per_call,
            subscription_rate_per_block,
            min_reputation_score,
            min_stake,
            capability_tags,
            status: bytes[136],
            reserved,
        })
    }
}

/// On-chain ZK verification key registration carried in a
/// `MemoryObjectType::VkRegistration` memory object (Week 30).
///
/// Publishers (AI entities that own ZK circuits) register a VK once; future
/// `ProofSubmission` signals carrying `PROOF_TYPE_GROTH16_REGISTERED` (and
/// other registered-variants) reference this entry by the containing
/// memory object's 32-byte `object_id` instead of inlining the full VK
/// every time. The execution handler revalidates the proof against the
/// stored VK at submission time.
///
/// Binding to a single `code_hash` means a registered entry can only be
/// used by proofs claiming the matching computation; the
/// `ProofSubmission` handler rejects mismatched `code_hash`.
///
/// Wire layout (variable, minimum `VK_REGISTRATION_HEADER_SIZE` + 1 byte):
/// `version:1 | proof_type:1 | code_hash:32 | label_len:1 | vk_len_be:4 |
/// label:label_len | vk_bytes:vk_len`. Length-prefixed because VKs are
/// not fixed-size across proof systems; the `label_len` byte is forced
/// into `[0, 255]` by its u8 type but the handler additionally enforces
/// `label_len <= VK_REGISTRATION_LABEL_MAX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VkRegistrationData {
    /// Wire-format version byte. Must equal `VK_REGISTRATION_VERSION` at
    /// create/update; decode preserves it verbatim so the handler can
    /// surface a specific error on mismatch.
    pub version: u8,
    /// Proof-system discriminant identifying which verifier this VK
    /// belongs to. Values use the `PROOF_TYPE_*` constants defined in
    /// `crates/execution/src/lib.rs`. The create handler accepts only
    /// supported variants; PLONK is reserved for a future activation.
    pub proof_type: u8,
    /// Canonical `code_hash` (AI module / circuit identity) this VK
    /// verifies. Bound at registration; the `ProofSubmission` handler
    /// requires the submitted proof's `code_hash` to equal this value
    /// when it resolves the VK through the registry.
    pub code_hash: [u8; 32],
    /// Free-form UTF-8 label (max `VK_REGISTRATION_LABEL_MAX` bytes).
    /// Empty by convention if the publisher chose not to annotate the
    /// registration. Mutable via `UpdateMemoryObject` so publishers can
    /// refine the human-readable tag without re-registering the key.
    pub label: Vec<u8>,
    /// Compressed verification-key bytes (ark-serialize compressed form
    /// for the BN254 Groth16 verifier). Length is bounded by the
    /// handler against the same per-payload cap that gates inline VK
    /// bytes in v2 `ProofSubmission` payloads. Immutable via
    /// `UpdateMemoryObject`.
    pub vk_bytes: Vec<u8>,
}

impl VkRegistrationData {
    /// Encode this registration to its canonical variable-length form.
    ///
    /// The encoder is total: oversized `label` or `vk_bytes` are
    /// silently saturated at `u8::MAX` / `u32::MAX` respectively, which
    /// matches the convention that handler-level validation runs before
    /// encode. Callers must validate length caps via the handler before
    /// trusting roundtrip equality.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let label_len = u8::try_from(self.label.len()).unwrap_or(u8::MAX);
        let vk_len = u32::try_from(self.vk_bytes.len()).unwrap_or(u32::MAX);
        let mut out =
            Vec::with_capacity(VK_REGISTRATION_HEADER_SIZE + label_len as usize + vk_len as usize);
        out.push(self.version);
        out.push(self.proof_type);
        out.extend_from_slice(&self.code_hash);
        out.push(label_len);
        out.extend_from_slice(&vk_len.to_be_bytes());
        out.extend_from_slice(&self.label[..label_len as usize]);
        out.extend_from_slice(&self.vk_bytes[..vk_len as usize]);
        out
    }

    /// Decode a VK registration from canonical bytes.
    ///
    /// Returns `None` if the input is shorter than
    /// `VK_REGISTRATION_HEADER_SIZE`, if `label_len + vk_len + header`
    /// would overflow `usize`, or if the total length does not match
    /// the prefixes exactly. Field-level validation (version, supported
    /// proof_type, label cap, VK byte cap, VK deserializability) is the
    /// handler's responsibility.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < VK_REGISTRATION_HEADER_SIZE {
            return None;
        }
        let version = bytes[0];
        let proof_type = bytes[1];
        let mut code_hash = [0u8; 32];
        code_hash.copy_from_slice(&bytes[2..34]);
        let label_len = bytes[34] as usize;
        let vk_len = u32::from_be_bytes(bytes[35..39].try_into().ok()?) as usize;
        let label_start = VK_REGISTRATION_HEADER_SIZE;
        let label_end = label_start.checked_add(label_len)?;
        let vk_end = label_end.checked_add(vk_len)?;
        if vk_end != bytes.len() {
            return None;
        }
        let label = bytes[label_start..label_end].to_vec();
        let vk_bytes = bytes[label_end..vk_end].to_vec();
        Some(Self {
            version,
            proof_type,
            code_hash,
            label,
            vk_bytes,
        })
    }
}

/// On-chain Service Level Agreement carried in a
/// `MemoryObjectType::SlaAgreement` memory object (Week 31).
///
/// Two-party agreement. The memory object owner is the BUYER who
/// proposed the agreement (via `CREATE_MEMORY_OBJECT`); the embedded
/// `seller_entity_id` accepts the agreement (via the `SlaAccept`
/// signal) and is the entity whose stake is at risk on threshold
/// breach. Once accepted the agreement is binding: there is no mutual
/// cancel signal in v1; the SLA runs until expiry or auto-termination
/// via threshold breach.
///
/// Wire layout (fixed 210 bytes; locked by golden vector test):
///
/// ```text
/// version:1
/// buyer_entity_id:32
/// seller_entity_id:32
/// service_descriptor_hash:32     (informational; zero = no reference)
/// status:1                       (SLA_STATUS_PROPOSED/ACTIVE/COMPLETED/VIOLATED/CANCELLED)
/// created_at_height_be:8
/// accepted_at_height_be:8        (0 until accepted)
/// start_height_be:8              (>= accepted_at_height; violation window opens here)
/// end_height_be:8                (> start_height; violation window closes here)
/// violation_count_be:4           (incremented on FAILED in-window attestations)
/// violation_threshold_be:4       (>= 1; immutable after create)
/// max_response_time_blocks_be:4  (RESERVED v1 - not enforced)
/// min_uptime_bps_be:2            (RESERVED v1 - not enforced; <= 10000)
/// min_delivery_success_bps_be:2  (RESERVED v1 - not enforced; <= 10000)
/// price_per_call_be:8            (informational; NAP enforces actual payments)
/// slash_amount_be:16             (single-shot penalty on threshold breach)
/// terminated_at_height_be:8      (0 until terminated)
/// slashed_amount_be:16           (actual debit; 0 until breach)
/// reserved:16                    (MUST be zero on create/update)
/// ```
///
/// Reserved fields (`max_response_time_blocks`, `min_uptime_bps`,
/// `min_delivery_success_bps`, `reserved[..16]`) reserve wire-format
/// real estate for future enforcement (response-time, uptime, success-
/// rate). The v1 runtime treats them as informational and validates
/// only that they decode within their type bounds and that `reserved`
/// is all-zero at create/update time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlaAgreementData {
    /// Wire-format version byte. Must equal `SLA_AGREEMENT_V1` at
    /// create / update; decode preserves it verbatim so the handler
    /// can surface a specific error on mismatch.
    pub version: u8,
    /// Buyer (memory object owner). Initiates the SLA via
    /// `CREATE_MEMORY_OBJECT`; can also issue
    /// `DELETE_MEMORY_OBJECT` on a still-Proposed agreement to
    /// cancel before acceptance.
    pub buyer_entity_id: [u8; 32],
    /// Seller (counterparty). Accepts the SLA via the `SlaAccept`
    /// signal (transitioning `SLA_STATUS_PROPOSED` -> `_ACTIVE`).
    /// Stake-bearer for the auto-slash path.
    pub seller_entity_id: [u8; 32],
    /// Optional reference to a Week 29 `ServiceDescriptor` memory
    /// object id describing the service being agreed on. Zero
    /// (all-zero bytes) signals "no reference". Informational only;
    /// the runtime does NOT verify that the hash resolves to a real
    /// `ServiceDescriptor`.
    pub service_descriptor_hash: [u8; 32],
    /// Lifecycle status discriminant. Must be `SLA_STATUS_PROPOSED`
    /// at create time; transitions to `_ACTIVE` on `SlaAccept` and
    /// to `_VIOLATED` on threshold breach. `_COMPLETED` and
    /// `_CANCELLED` are reserved discriminants that no v1 handler
    /// writes (expiry stays in `_ACTIVE` with `is_expired` derived
    /// at the RPC layer; cancellation deletes the memory object).
    pub status: u8,
    /// Block height the proposal landed on chain (set by the
    /// create handler from the block context).
    pub created_at_height: u64,
    /// Block height the seller's `SlaAccept` signal landed. 0
    /// while `status == SLA_STATUS_PROPOSED`. Set verbatim from
    /// the block context on acceptance.
    pub accepted_at_height: u64,
    /// Inclusive lower bound of the violation window. Must be
    /// `>= accepted_at_height` at acceptance time (enforced by the
    /// `SlaAccept` handler when set).
    pub start_height: u64,
    /// Inclusive upper bound of the violation window. Must be
    /// `> start_height` at create. `end_height - start_height
    /// <= SLA_MAX_DURATION_BLOCKS`.
    pub end_height: u64,
    /// Cumulative count of in-window `PAYMENT_ATTESTATION_STATUS_FAILED`
    /// `ServiceAttestation` signals seen against this SLA. Initial
    /// 0; saturating-increment. Auto-slash fires when this value
    /// `>= violation_threshold` (one-shot terminal transition).
    pub violation_count: u32,
    /// Number of in-window FAILED attestations that must accumulate
    /// before auto-slash fires. Immutable after create. `>= 1`.
    pub violation_threshold: u32,
    /// RESERVED v1. Maximum acceptable response time in blocks.
    /// Not enforced by the v1 runtime; included for forward
    /// compatibility with a future off-chain response-time oracle.
    pub max_response_time_blocks: u32,
    /// RESERVED v1. Minimum acceptable uptime ratio in basis points
    /// (10 000 = 100%). Not enforced by the v1 runtime; validated
    /// only against the bps range cap.
    pub min_uptime_bps: u16,
    /// RESERVED v1. Minimum acceptable delivery success ratio in
    /// basis points. Not enforced by the v1 runtime; validated
    /// only against the bps range cap.
    pub min_delivery_success_bps: u16,
    /// Informational: per-call price the buyer expects to pay for
    /// services covered by this SLA. The actual payment flow is
    /// handled by Week 28 `PaymentRequest` signals; this field is
    /// metadata for off-chain consumers.
    pub price_per_call: u64,
    /// Penalty debited from the seller's `stake_balance` on
    /// auto-slash (`min(stake_balance, slash_amount)` to mirror
    /// `StakeSlash` semantics). Must be `> 0` at create.
    pub slash_amount: u128,
    /// Block height the SLA transitioned to a terminal state. 0
    /// until breach or cancellation. Set verbatim from the block
    /// context.
    pub terminated_at_height: u64,
    /// Actual debit applied on auto-slash. 0 until breach. May be
    /// less than `slash_amount` if the seller's stake balance was
    /// below `slash_amount` at breach time (saturating).
    pub slashed_amount: u128,
    /// 16 bytes reserved for future field allocation. MUST be zero
    /// on create / update; decode preserves verbatim so future
    /// schema additions are forward-compatible with the v1 binary
    /// layout.
    pub reserved: [u8; SLA_RESERVED_LEN],
}

impl SlaAgreementData {
    /// Encode this SLA agreement to its 210-byte canonical form.
    #[must_use]
    pub fn encode(&self) -> [u8; SLA_AGREEMENT_SIZE] {
        let mut out = [0u8; SLA_AGREEMENT_SIZE];
        out[0] = self.version;
        out[1..33].copy_from_slice(&self.buyer_entity_id);
        out[33..65].copy_from_slice(&self.seller_entity_id);
        out[65..97].copy_from_slice(&self.service_descriptor_hash);
        out[97] = self.status;
        out[98..106].copy_from_slice(&self.created_at_height.to_be_bytes());
        out[106..114].copy_from_slice(&self.accepted_at_height.to_be_bytes());
        out[114..122].copy_from_slice(&self.start_height.to_be_bytes());
        out[122..130].copy_from_slice(&self.end_height.to_be_bytes());
        out[130..134].copy_from_slice(&self.violation_count.to_be_bytes());
        out[134..138].copy_from_slice(&self.violation_threshold.to_be_bytes());
        out[138..142].copy_from_slice(&self.max_response_time_blocks.to_be_bytes());
        out[142..144].copy_from_slice(&self.min_uptime_bps.to_be_bytes());
        out[144..146].copy_from_slice(&self.min_delivery_success_bps.to_be_bytes());
        out[146..154].copy_from_slice(&self.price_per_call.to_be_bytes());
        out[154..170].copy_from_slice(&self.slash_amount.to_be_bytes());
        out[170..178].copy_from_slice(&self.terminated_at_height.to_be_bytes());
        out[178..194].copy_from_slice(&self.slashed_amount.to_be_bytes());
        out[194..210].copy_from_slice(&self.reserved);
        out
    }

    /// Decode an SLA agreement from canonical bytes.
    ///
    /// Returns `None` if the slice length is not exactly
    /// `SLA_AGREEMENT_SIZE`. Field-level validation (version,
    /// status range, window ordering, bps range, zero-reserved,
    /// threshold non-zero, slash-amount non-zero) is the handler's
    /// responsibility; decode preserves byte content verbatim so
    /// the runtime can produce specific error variants for each
    /// invalid field.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SLA_AGREEMENT_SIZE {
            return None;
        }
        let mut buyer_entity_id = [0u8; 32];
        buyer_entity_id.copy_from_slice(&bytes[1..33]);
        let mut seller_entity_id = [0u8; 32];
        seller_entity_id.copy_from_slice(&bytes[33..65]);
        let mut service_descriptor_hash = [0u8; 32];
        service_descriptor_hash.copy_from_slice(&bytes[65..97]);
        let created_at_height = u64::from_be_bytes(bytes[98..106].try_into().ok()?);
        let accepted_at_height = u64::from_be_bytes(bytes[106..114].try_into().ok()?);
        let start_height = u64::from_be_bytes(bytes[114..122].try_into().ok()?);
        let end_height = u64::from_be_bytes(bytes[122..130].try_into().ok()?);
        let violation_count = u32::from_be_bytes(bytes[130..134].try_into().ok()?);
        let violation_threshold = u32::from_be_bytes(bytes[134..138].try_into().ok()?);
        let max_response_time_blocks = u32::from_be_bytes(bytes[138..142].try_into().ok()?);
        let min_uptime_bps = u16::from_be_bytes(bytes[142..144].try_into().ok()?);
        let min_delivery_success_bps = u16::from_be_bytes(bytes[144..146].try_into().ok()?);
        let price_per_call = u64::from_be_bytes(bytes[146..154].try_into().ok()?);
        let slash_amount = u128::from_be_bytes(bytes[154..170].try_into().ok()?);
        let terminated_at_height = u64::from_be_bytes(bytes[170..178].try_into().ok()?);
        let slashed_amount = u128::from_be_bytes(bytes[178..194].try_into().ok()?);
        let mut reserved = [0u8; SLA_RESERVED_LEN];
        reserved.copy_from_slice(&bytes[194..210]);
        Some(Self {
            version: bytes[0],
            buyer_entity_id,
            seller_entity_id,
            service_descriptor_hash,
            status: bytes[97],
            created_at_height,
            accepted_at_height,
            start_height,
            end_height,
            violation_count,
            violation_threshold,
            max_response_time_blocks,
            min_uptime_bps,
            min_delivery_success_bps,
            price_per_call,
            slash_amount,
            terminated_at_height,
            slashed_amount,
            reserved,
        })
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
            MemoryObjectType::CompositionGraph,
            MemoryObjectType::DelegationGrant,
            MemoryObjectType::Subscription,
            MemoryObjectType::ServiceDescriptor,
            MemoryObjectType::VkRegistration,
            MemoryObjectType::SlaAgreement,
        ] {
            let byte = t.to_byte();
            let decoded = MemoryObjectType::from_byte(byte).unwrap();
            assert_eq!(t, decoded, "Type {t:?} roundtrip failed");
        }
    }

    #[test]
    fn memory_object_type_invalid_returns_none() {
        assert_eq!(
            MemoryObjectType::from_byte(13),
            Some(MemoryObjectType::VkRegistration),
            "byte 13 must decode to VkRegistration (Week 30)"
        );
        assert_eq!(
            MemoryObjectType::from_byte(14),
            Some(MemoryObjectType::SlaAgreement),
            "byte 14 must decode to SlaAgreement (Week 31)"
        );
        assert!(
            MemoryObjectType::from_byte(15).is_none(),
            "15 is the first invalid byte after Week 31"
        );
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
        assert_eq!(
            MemoryObjectType::CompositionGraph.name(),
            "CompositionGraph"
        );
        assert_eq!(MemoryObjectType::DelegationGrant.name(), "DelegationGrant");
        assert_eq!(MemoryObjectType::Subscription.name(), "Subscription");
        assert_eq!(
            MemoryObjectType::ServiceDescriptor.name(),
            "ServiceDescriptor"
        );
        assert_eq!(MemoryObjectType::VkRegistration.name(), "VkRegistration");
        assert_eq!(MemoryObjectType::SlaAgreement.name(), "SlaAgreement");
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
            MemoryObjectType::CompositionGraph,
            MemoryObjectType::VerificationRecord,
            MemoryObjectType::DelegationGrant,
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

    // ========================================================================
    // CompositionGraph (Feature 4) tests
    // ========================================================================

    fn sample_dep(source: [u8; 32], sig_type: u8, is_required: bool) -> CompositionDependency {
        CompositionDependency {
            source_entity_id: source,
            required_signal_type: sig_type,
            min_reputation: 50,
            min_stake: 1_000,
            is_required,
        }
    }

    #[test]
    fn memory_object_type_composition_graph_byte() {
        assert_eq!(MemoryObjectType::CompositionGraph.to_byte(), 8);
        assert_eq!(
            MemoryObjectType::from_byte(8),
            Some(MemoryObjectType::CompositionGraph)
        );
        assert_eq!(
            MemoryObjectType::from_byte(9),
            Some(MemoryObjectType::VerificationRecord)
        );
        assert_eq!(
            MemoryObjectType::from_byte(10),
            Some(MemoryObjectType::DelegationGrant)
        );
        assert_eq!(
            MemoryObjectType::from_byte(11),
            Some(MemoryObjectType::Subscription)
        );
        assert_eq!(
            MemoryObjectType::from_byte(12),
            Some(MemoryObjectType::ServiceDescriptor)
        );
        assert_eq!(
            MemoryObjectType::from_byte(13),
            Some(MemoryObjectType::VkRegistration)
        );
        assert_eq!(
            MemoryObjectType::from_byte(14),
            Some(MemoryObjectType::SlaAgreement)
        );
        assert_eq!(MemoryObjectType::from_byte(15), None);
        assert_eq!(
            MemoryObjectType::CompositionGraph.name(),
            "CompositionGraph"
        );
        assert_eq!(
            MemoryObjectType::VerificationRecord.name(),
            "VerificationRecord"
        );
        assert_eq!(MemoryObjectType::DelegationGrant.name(), "DelegationGrant");
        assert_eq!(MemoryObjectType::Subscription.name(), "Subscription");
    }

    #[test]
    fn composition_dependency_byte_layout() {
        let dep = CompositionDependency {
            source_entity_id: [0xAAu8; 32],
            required_signal_type: 2,
            min_reputation: 0x1234,
            min_stake: 0x0102_0304_0506_0708,
            is_required: true,
        };
        let bytes = dep.encode();
        assert_eq!(bytes.len(), COMPOSITION_DEPENDENCY_SIZE);
        assert_eq!(bytes.len(), 44);
        assert_eq!(&bytes[0..32], &[0xAAu8; 32], "source_entity_id at 0..32");
        assert_eq!(bytes[32], 2, "required_signal_type at 32");
        assert_eq!(
            &bytes[33..35],
            &0x1234u16.to_be_bytes(),
            "min_reputation BE at 33..35"
        );
        assert_eq!(
            &bytes[35..43],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "min_stake BE at 35..43"
        );
        assert_eq!(bytes[43], 1, "is_required at 43 (1 = true)");
    }

    #[test]
    fn composition_dependency_roundtrip() {
        let cases = [
            CompositionDependency {
                source_entity_id: [0u8; 32],
                required_signal_type: 0,
                min_reputation: 0,
                min_stake: 0,
                is_required: false,
            },
            CompositionDependency {
                source_entity_id: [0xFFu8; 32],
                required_signal_type: u8::MAX,
                min_reputation: u16::MAX,
                min_stake: u64::MAX,
                is_required: true,
            },
            sample_dep([0x42u8; 32], 7, false),
        ];
        for c in &cases {
            let encoded = c.encode();
            let decoded = CompositionDependency::decode(&encoded).expect("decode");
            assert_eq!(&decoded, c);
        }
    }

    #[test]
    fn composition_dependency_decode_invalid_is_required_byte() {
        let mut bytes = [0u8; COMPOSITION_DEPENDENCY_SIZE];
        bytes[43] = 2;
        assert!(CompositionDependency::decode(&bytes).is_none());
    }

    #[test]
    fn composition_dependency_decode_too_short() {
        let bytes = [0u8; COMPOSITION_DEPENDENCY_SIZE - 1];
        assert!(CompositionDependency::decode(&bytes).is_none());
    }

    #[test]
    fn composition_graph_data_empty_roundtrip() {
        let g = CompositionGraphData::default();
        let encoded = g.encode();
        assert_eq!(encoded, vec![0u8]);
        let decoded = CompositionGraphData::decode(&encoded).expect("decode");
        assert_eq!(decoded, g);
    }

    #[test]
    fn composition_graph_data_single_entry_roundtrip() {
        let dep = sample_dep([0x12u8; 32], 3, true);
        let g = CompositionGraphData {
            dependencies: vec![dep],
        };
        let encoded = g.encode();
        assert_eq!(encoded.len(), 1 + COMPOSITION_DEPENDENCY_SIZE);
        let decoded = CompositionGraphData::decode(&encoded).expect("decode");
        assert_eq!(decoded, g);
    }

    #[test]
    fn composition_graph_data_full_capacity_roundtrip() {
        let mut deps = Vec::with_capacity(MAX_COMPOSITION_DEPENDENCIES);
        for i in 0..MAX_COMPOSITION_DEPENDENCIES {
            // Ensure unique (source_entity_id, required_signal_type) pairs.
            let mut id = [0u8; 32];
            id[0] = i as u8;
            deps.push(CompositionDependency {
                source_entity_id: id,
                required_signal_type: i as u8,
                min_reputation: i as u16,
                min_stake: 1_000 * (i as u64 + 1),
                is_required: i % 2 == 0,
            });
        }
        let g = CompositionGraphData { dependencies: deps };
        let encoded = g.encode();
        assert_eq!(
            encoded.len(),
            1 + MAX_COMPOSITION_DEPENDENCIES * COMPOSITION_DEPENDENCY_SIZE
        );
        assert_eq!(encoded.len(), COMPOSITION_GRAPH_MAX_SIZE);
        assert_eq!(encoded.len(), 441);
        let decoded = CompositionGraphData::decode(&encoded).expect("decode");
        assert_eq!(decoded, g);
        assert_eq!(decoded.dependencies.len(), MAX_COMPOSITION_DEPENDENCIES);
    }

    #[test]
    fn composition_graph_data_decode_count_too_large() {
        let mut bytes =
            vec![0u8; 1 + (MAX_COMPOSITION_DEPENDENCIES + 1) * COMPOSITION_DEPENDENCY_SIZE];
        bytes[0] = (MAX_COMPOSITION_DEPENDENCIES + 1) as u8;
        assert!(CompositionGraphData::decode(&bytes).is_none());
    }

    #[test]
    fn composition_graph_data_decode_truncated() {
        // Claim 2 deps but provide only 1 dep's worth of bytes after count.
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&[0u8; COMPOSITION_DEPENDENCY_SIZE]);
        assert!(CompositionGraphData::decode(&bytes).is_none());
    }

    #[test]
    fn composition_graph_data_decode_empty_buffer() {
        assert!(CompositionGraphData::decode(&[]).is_none());
    }

    #[test]
    fn composition_graph_data_encode_caps_at_max_dependencies() {
        let mut deps = Vec::new();
        for i in 0..(MAX_COMPOSITION_DEPENDENCIES + 5) {
            let mut id = [0u8; 32];
            id[0] = i as u8;
            deps.push(sample_dep(id, i as u8, false));
        }
        let g = CompositionGraphData { dependencies: deps };
        let encoded = g.encode();
        assert_eq!(encoded.len(), COMPOSITION_GRAPH_MAX_SIZE);
        assert_eq!(encoded[0], MAX_COMPOSITION_DEPENDENCIES as u8);
    }

    #[test]
    fn composition_graph_data_decode_rejects_duplicate_dependency() {
        // Two entries with identical (source_entity_id, required_signal_type)
        // must be rejected at decode time.
        let dup_source = [0x77u8; 32];
        let dep_a = CompositionDependency {
            source_entity_id: dup_source,
            required_signal_type: 4,
            min_reputation: 10,
            min_stake: 100,
            is_required: true,
        };
        let dep_b = CompositionDependency {
            source_entity_id: dup_source,
            required_signal_type: 4,
            min_reputation: 99,
            min_stake: 999,
            is_required: false,
        };
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&dep_a.encode());
        bytes.extend_from_slice(&dep_b.encode());
        assert!(CompositionGraphData::decode(&bytes).is_none());
    }

    #[test]
    fn composition_graph_data_decode_allows_same_source_different_signal() {
        // Same source_entity_id but different required_signal_type is fine —
        // the owner consumes two distinct signal kinds from one source.
        let source = [0x42u8; 32];
        let dep_a = sample_dep(source, 1, false);
        let dep_b = sample_dep(source, 2, false);
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&dep_a.encode());
        bytes.extend_from_slice(&dep_b.encode());
        let decoded = CompositionGraphData::decode(&bytes).expect("decode");
        assert_eq!(decoded.dependencies.len(), 2);
    }

    // ========================================================================
    // VerificationRecordData tests
    // ========================================================================

    #[test]
    fn verification_record_data_roundtrip() {
        let record = VerificationRecordData {
            proof_type: 0,
            code_hash: [0xA1u8; 32],
            computation_hash: [0xB2u8; 32],
            proof_hash: [0xC3u8; 32],
            height: 1_234_567,
        };
        let encoded = record.encode();
        assert_eq!(encoded.len(), VERIFICATION_RECORD_SIZE);
        assert_eq!(encoded.len(), 105);
        let decoded = VerificationRecordData::decode(&encoded).expect("decode");
        assert_eq!(decoded, record);
    }

    #[test]
    fn verification_record_data_byte_layout_is_frozen() {
        // Frozen field offsets — moving any of these is a wire-format break.
        let record = VerificationRecordData {
            proof_type: 7,
            code_hash: [0x11u8; 32],
            computation_hash: [0x22u8; 32],
            proof_hash: [0x33u8; 32],
            height: 0x0102_0304_0506_0708,
        };
        let encoded = record.encode();
        assert_eq!(encoded[0], 7, "proof_type at byte 0");
        assert_eq!(&encoded[1..33], &[0x11u8; 32], "code_hash at 1..33");
        assert_eq!(
            &encoded[33..65],
            &[0x22u8; 32],
            "computation_hash at 33..65"
        );
        assert_eq!(&encoded[65..97], &[0x33u8; 32], "proof_hash at 65..97");
        assert_eq!(
            &encoded[97..105],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "height_be at 97..105"
        );
    }

    #[test]
    fn verification_record_data_decode_too_short() {
        let bytes = vec![0u8; VERIFICATION_RECORD_SIZE - 1];
        assert!(VerificationRecordData::decode(&bytes).is_none());
    }

    #[test]
    fn verification_record_data_decode_too_long() {
        let bytes = vec![0u8; VERIFICATION_RECORD_SIZE + 1];
        assert!(VerificationRecordData::decode(&bytes).is_none());
    }

    #[test]
    fn verification_record_data_decode_empty_returns_none() {
        assert!(VerificationRecordData::decode(&[]).is_none());
    }

    // ========================================================================
    // DelegationGrant (Feature 8) tests
    // ========================================================================

    fn sample_grant() -> DelegationGrantData {
        DelegationGrantData {
            version: DELEGATION_GRANT_VERSION,
            delegate_entity_id: [0xAAu8; 32],
            granted_capabilities: 0x04, // emit_proposals
            expires_at: 1_000_000,
        }
    }

    #[test]
    fn delegation_grant_data_byte_layout() {
        let g = DelegationGrantData {
            version: DELEGATION_GRANT_VERSION,
            delegate_entity_id: [0xBBu8; 32],
            granted_capabilities: 0x27,
            expires_at: 0x0102_0304_0506_0708,
        };
        let encoded = g.encode();
        assert_eq!(encoded.len(), DELEGATION_GRANT_SIZE);
        assert_eq!(encoded.len(), 42);
        assert_eq!(encoded[0], DELEGATION_GRANT_VERSION, "version at 0");
        assert_eq!(
            &encoded[1..33],
            &[0xBBu8; 32],
            "delegate_entity_id at 1..33"
        );
        assert_eq!(encoded[33], 0x27, "granted_capabilities at 33");
        assert_eq!(
            &encoded[34..42],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "expires_at_be at 34..42"
        );
    }

    #[test]
    fn delegation_grant_data_roundtrip() {
        let cases = [
            sample_grant(),
            DelegationGrantData {
                version: DELEGATION_GRANT_VERSION,
                delegate_entity_id: [0u8; 32],
                granted_capabilities: 0,
                expires_at: 0,
            },
            DelegationGrantData {
                version: DELEGATION_GRANT_VERSION,
                delegate_entity_id: [0xFFu8; 32],
                granted_capabilities: u8::MAX,
                expires_at: u64::MAX,
            },
        ];
        for g in cases {
            let encoded = g.encode();
            let decoded = DelegationGrantData::decode(&encoded).expect("decode");
            assert_eq!(g, decoded, "roundtrip {g:?}");
        }
    }

    #[test]
    fn delegation_grant_data_decode_wrong_size() {
        let too_short = vec![0u8; DELEGATION_GRANT_SIZE - 1];
        assert!(DelegationGrantData::decode(&too_short).is_none());
        let too_long = vec![0u8; DELEGATION_GRANT_SIZE + 1];
        assert!(DelegationGrantData::decode(&too_long).is_none());
        assert!(DelegationGrantData::decode(&[]).is_none());
    }

    #[test]
    fn delegation_grant_data_decode_bad_version() {
        let mut bytes = sample_grant().encode();
        bytes[0] = DELEGATION_GRANT_VERSION + 1;
        assert!(DelegationGrantData::decode(&bytes).is_none());
        bytes[0] = 0;
        assert!(DelegationGrantData::decode(&bytes).is_none());
    }

    #[test]
    fn delegation_grant_data_is_active_at_no_expiry() {
        let g = DelegationGrantData {
            expires_at: 0,
            ..sample_grant()
        };
        assert!(g.is_active_at(0));
        assert!(g.is_active_at(u64::MAX));
    }

    #[test]
    fn delegation_grant_data_is_active_at_with_expiry() {
        let g = DelegationGrantData {
            expires_at: 100,
            ..sample_grant()
        };
        assert!(g.is_active_at(0));
        assert!(g.is_active_at(99));
        assert!(!g.is_active_at(100), "expires_at is exclusive upper bound");
        assert!(!g.is_active_at(101));
    }

    #[test]
    fn delegation_grant_via_memory_object_envelope_roundtrip() {
        let owner = [0x42u8; 32];
        let grant = sample_grant();
        let obj = MemoryObject::new(
            owner,
            MemoryObjectType::DelegationGrant,
            5_000,
            grant.encode().to_vec(),
        );
        let encoded = encode_memory_object_v1(&obj);
        let decoded = decode_memory_object_v1(&encoded).expect("envelope decode");
        assert_eq!(decoded.object_type, MemoryObjectType::DelegationGrant);
        let payload = DelegationGrantData::decode(&decoded.data).expect("payload decode");
        assert_eq!(payload, grant);
    }

    #[test]
    fn max_delegation_grants_constant() {
        assert_eq!(MAX_DELEGATION_GRANTS, 20);
    }

    // ========================================================================
    // Subscription (Feature 9) tests
    // ========================================================================

    fn sample_subscription() -> SubscriptionData {
        SubscriptionData {
            subscriber_entity_id: [0xAAu8; 32],
            producer_entity_id: [0xBBu8; 32],
            covered_signal_type: 2, // Prediction
            rate_per_block: 10,
            start_height: 1_000,
            end_height: 11_000,
            last_settled_height: 1_000,
            total_locked: 100_000,
            is_active: true,
        }
    }

    #[test]
    fn subscription_data_byte_layout() {
        let s = SubscriptionData {
            subscriber_entity_id: [0x11u8; 32],
            producer_entity_id: [0x22u8; 32],
            covered_signal_type: 0x07,
            rate_per_block: 0x0102_0304_0506_0708,
            start_height: 0x1112_1314_1516_1718,
            end_height: 0x2122_2324_2526_2728,
            last_settled_height: 0x3132_3334_3536_3738,
            total_locked: 0x4142_4344_4546_4748_5152_5354_5556_5758,
            is_active: true,
        };
        let encoded = s.encode();
        assert_eq!(encoded.len(), SUBSCRIPTION_SIZE);
        assert_eq!(encoded.len(), 114);
        assert_eq!(
            &encoded[0..32],
            &[0x11u8; 32],
            "subscriber_entity_id at 0..32"
        );
        assert_eq!(
            &encoded[32..64],
            &[0x22u8; 32],
            "producer_entity_id at 32..64"
        );
        assert_eq!(encoded[64], 0x07, "covered_signal_type at 64");
        assert_eq!(
            &encoded[65..73],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "rate_per_block_be at 65..73"
        );
        assert_eq!(
            &encoded[73..81],
            &0x1112_1314_1516_1718u64.to_be_bytes(),
            "start_height_be at 73..81"
        );
        assert_eq!(
            &encoded[81..89],
            &0x2122_2324_2526_2728u64.to_be_bytes(),
            "end_height_be at 81..89"
        );
        assert_eq!(
            &encoded[89..97],
            &0x3132_3334_3536_3738u64.to_be_bytes(),
            "last_settled_height_be at 89..97"
        );
        assert_eq!(
            &encoded[97..113],
            &0x4142_4344_4546_4748_5152_5354_5556_5758u128.to_be_bytes(),
            "total_locked_be at 97..113"
        );
        assert_eq!(encoded[113], 1, "is_active at 113 (1 = true)");
    }

    #[test]
    fn subscription_data_roundtrip() {
        let cases = [
            sample_subscription(),
            SubscriptionData {
                subscriber_entity_id: [0u8; 32],
                producer_entity_id: [0u8; 32],
                covered_signal_type: 0,
                rate_per_block: 0,
                start_height: 0,
                end_height: 0,
                last_settled_height: 0,
                total_locked: 0,
                is_active: false,
            },
            SubscriptionData {
                subscriber_entity_id: [0xFFu8; 32],
                producer_entity_id: [0xFFu8; 32],
                covered_signal_type: u8::MAX,
                rate_per_block: u64::MAX,
                start_height: u64::MAX,
                end_height: u64::MAX,
                last_settled_height: u64::MAX,
                total_locked: u128::MAX,
                is_active: true,
            },
            SubscriptionData {
                is_active: false,
                ..sample_subscription()
            },
        ];
        for s in cases {
            let encoded = s.encode();
            let decoded = SubscriptionData::decode(&encoded).expect("decode");
            assert_eq!(s, decoded, "roundtrip {s:?}");
        }
    }

    #[test]
    fn subscription_data_decode_wrong_size() {
        let too_short = vec![0u8; SUBSCRIPTION_SIZE - 1];
        assert!(SubscriptionData::decode(&too_short).is_none());
        let too_long = vec![0u8; SUBSCRIPTION_SIZE + 1];
        assert!(SubscriptionData::decode(&too_long).is_none());
        assert!(SubscriptionData::decode(&[]).is_none());
    }

    #[test]
    fn subscription_data_decode_invalid_is_active_byte() {
        let mut bytes = sample_subscription().encode();
        bytes[113] = 2;
        assert!(SubscriptionData::decode(&bytes).is_none());
        bytes[113] = u8::MAX;
        assert!(SubscriptionData::decode(&bytes).is_none());
    }

    #[test]
    fn subscription_data_settlable_blocks_inside_window() {
        let mut s = sample_subscription();
        s.last_settled_height = 1_000;
        // Halfway through the 10_000-block window.
        assert_eq!(s.settlable_blocks(6_000), 5_000);
    }

    #[test]
    fn subscription_data_settlable_blocks_capped_at_end_height() {
        let s = sample_subscription();
        // current_height beyond end_height: settle no further than end_height.
        assert_eq!(s.settlable_blocks(20_000), 10_000);
    }

    #[test]
    fn subscription_data_settlable_blocks_zero_when_inactive() {
        let s = SubscriptionData {
            is_active: false,
            ..sample_subscription()
        };
        assert_eq!(s.settlable_blocks(6_000), 0);
    }

    #[test]
    fn subscription_data_settlable_blocks_saturating_when_height_below_settled() {
        let mut s = sample_subscription();
        s.last_settled_height = 5_000;
        // current_height before last_settled_height: saturating_sub returns 0.
        assert_eq!(s.settlable_blocks(2_000), 0);
    }

    #[test]
    fn subscription_data_accrued_gross_basic() {
        let mut s = sample_subscription();
        s.last_settled_height = 1_000;
        s.rate_per_block = 7;
        // 1_000 blocks at rate 7 = 7_000.
        assert_eq!(s.accrued_gross(2_000), Some(7_000));
    }

    #[test]
    fn subscription_data_accrued_gross_extreme_values_fit_in_u128() {
        let s = SubscriptionData {
            rate_per_block: u64::MAX,
            start_height: 0,
            end_height: u64::MAX,
            last_settled_height: 0,
            ..sample_subscription()
        };
        // u64::MAX blocks * u64::MAX rate is (2^64 - 1)^2, which fits in u128
        // (just under u128::MAX). The checked_mul therefore returns Some.
        // This documents that accrued_gross cannot overflow in v1 because
        // both operands are u64; the Option return type is defensive cover
        // for future widening of either operand.
        let blocks = u128::from(u64::MAX);
        let rate = u128::from(u64::MAX);
        let expected = blocks * rate;
        assert_eq!(s.accrued_gross(u64::MAX), Some(expected));
    }

    #[test]
    fn subscription_via_memory_object_envelope_roundtrip() {
        let owner = [0x42u8; 32];
        let sub = sample_subscription();
        let obj = MemoryObject::new(
            owner,
            MemoryObjectType::Subscription,
            sub.start_height,
            sub.encode().to_vec(),
        );
        let encoded = encode_memory_object_v1(&obj);
        let decoded = decode_memory_object_v1(&encoded).expect("envelope decode");
        assert_eq!(decoded.object_type, MemoryObjectType::Subscription);
        let payload = SubscriptionData::decode(&decoded.data).expect("payload decode");
        assert_eq!(payload, sub);
    }

    #[test]
    fn max_subscriptions_per_entity_constant() {
        assert_eq!(MAX_SUBSCRIPTIONS_PER_ENTITY, 10);
    }

    #[test]
    fn subscription_size_constant_matches_layout() {
        assert_eq!(SUBSCRIPTION_SIZE, 114);
        assert_eq!(SUBSCRIPTION_SIZE, 32 + 32 + 1 + 8 + 8 + 8 + 8 + 16 + 1);
    }

    // ========================================================================
    // Week 29 Phase 1: Agent Discovery Registry types and codec
    // ========================================================================

    fn sample_service_descriptor() -> ServiceDescriptorData {
        ServiceDescriptorData {
            version: SERVICE_DESCRIPTOR_V1,
            service_name_hash: [0xAAu8; 32],
            service_url_hash: [0xBBu8; 32],
            description_hash: [0xCCu8; 32],
            category: SERVICE_CATEGORY_DATA_ORACLE,
            price_per_call: 0x0102_0304_0506_0708,
            subscription_rate_per_block: 0x1112_1314_1516_1718,
            min_reputation_score: 50,
            min_stake: 0x2122_2324_2526_2728_2A2B_2C2D_2E2F_3031,
            capability_tags: 0x4142_4344,
            status: SERVICE_STATUS_ACTIVE,
            reserved: [0u8; 7],
        }
    }

    #[test]
    fn service_descriptor_size_constant_matches_layout() {
        assert_eq!(SERVICE_DESCRIPTOR_SIZE, 144);
        assert_eq!(
            SERVICE_DESCRIPTOR_SIZE,
            1 + 32 + 32 + 32 + 1 + 8 + 8 + 2 + 16 + 4 + 1 + 7
        );
    }

    #[test]
    fn service_descriptor_constants_are_stable() {
        assert_eq!(MAX_SERVICE_DESCRIPTORS_PER_ENTITY, 16);
        assert_eq!(SERVICE_DESCRIPTOR_V1, 1);

        // Category enum is pinned so callers can rely on byte values.
        assert_eq!(SERVICE_CATEGORY_GENERIC, 0);
        assert_eq!(SERVICE_CATEGORY_DATA_ORACLE, 1);
        assert_eq!(SERVICE_CATEGORY_INFERENCE, 2);
        assert_eq!(SERVICE_CATEGORY_COMPUTE, 3);
        assert_eq!(SERVICE_CATEGORY_STORAGE, 4);
        assert_eq!(SERVICE_CATEGORY_INDEXER, 5);
        assert_eq!(SERVICE_CATEGORY_SIGNAL_PROVIDER, 6);
        assert_eq!(SERVICE_CATEGORY_VERIFICATION, 7);
        assert_eq!(SERVICE_CATEGORY_MONITORING, 8);
        assert_eq!(SERVICE_CATEGORY_GATEWAY, 9);
        assert_eq!(SERVICE_CATEGORY_RESERVED_MAX, 15);

        // Status enum is pinned so the handler's status checks line up
        // with the constants regardless of refactor ordering.
        assert_eq!(SERVICE_STATUS_ACTIVE, 0);
        assert_eq!(SERVICE_STATUS_PAUSED, 1);
        assert_eq!(SERVICE_STATUS_DEPRECATED, 2);
        assert_eq!(SERVICE_STATUS_MAX, SERVICE_STATUS_DEPRECATED);
    }

    #[test]
    fn service_descriptor_roundtrip() {
        let sd = sample_service_descriptor();
        let bytes = sd.encode();
        assert_eq!(bytes.len(), SERVICE_DESCRIPTOR_SIZE);
        let decoded = ServiceDescriptorData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded, sd);
    }

    #[test]
    fn service_descriptor_roundtrip_preserves_arbitrary_reserved_bytes() {
        // The decoder is byte-faithful: any value in `reserved` survives
        // a roundtrip. Handler-level validation (zero-reserved on
        // create/update) is a runtime rule, NOT a codec rule, so the
        // decoder must not fail on a non-zero `reserved`. This test
        // locks that contract so future schema additions stay backward
        // compatible at the byte level.
        let mut sd = sample_service_descriptor();
        sd.reserved = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];
        let bytes = sd.encode();
        let decoded = ServiceDescriptorData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.reserved, sd.reserved);
    }

    #[test]
    fn service_descriptor_decode_rejects_wrong_length() {
        let sd = sample_service_descriptor();
        let mut bytes = sd.encode().to_vec();
        bytes.push(0);
        assert!(ServiceDescriptorData::decode(&bytes).is_none());
        bytes.pop();
        bytes.pop();
        assert!(ServiceDescriptorData::decode(&bytes).is_none());
    }

    #[test]
    fn service_descriptor_decode_preserves_version_byte() {
        // Handler validates version; decoder does not. Byte 0 is
        // preserved verbatim so the runtime can surface a specific
        // InvalidServiceDescriptor error on mismatch.
        let mut sd = sample_service_descriptor();
        sd.version = 99;
        let bytes = sd.encode();
        let decoded = ServiceDescriptorData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.version, 99);
    }

    #[test]
    fn golden_vector_service_descriptor_144_bytes() {
        let sd = sample_service_descriptor();
        let bytes = sd.encode();
        assert_eq!(bytes.len(), 144);

        assert_eq!(bytes[0], SERVICE_DESCRIPTOR_V1, "version at 0");
        assert_eq!(&bytes[1..33], &[0xAAu8; 32], "service_name_hash at 1..33");
        assert_eq!(&bytes[33..65], &[0xBBu8; 32], "service_url_hash at 33..65");
        assert_eq!(&bytes[65..97], &[0xCCu8; 32], "description_hash at 65..97");
        assert_eq!(bytes[97], SERVICE_CATEGORY_DATA_ORACLE, "category at 97");
        assert_eq!(
            &bytes[98..106],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "price_per_call_be at 98..106"
        );
        assert_eq!(
            &bytes[106..114],
            &0x1112_1314_1516_1718u64.to_be_bytes(),
            "subscription_rate_per_block_be at 106..114"
        );
        assert_eq!(
            &bytes[114..116],
            &50u16.to_be_bytes(),
            "min_reputation_score_be at 114..116"
        );
        assert_eq!(
            &bytes[116..132],
            &0x2122_2324_2526_2728_2A2B_2C2D_2E2F_3031u128.to_be_bytes(),
            "min_stake_be at 116..132"
        );
        assert_eq!(
            &bytes[132..136],
            &0x4142_4344u32.to_be_bytes(),
            "capability_tags_be at 132..136"
        );
        assert_eq!(bytes[136], SERVICE_STATUS_ACTIVE, "status at 136");
        assert_eq!(&bytes[137..144], &[0u8; 7], "reserved at 137..144 (zero)");
    }

    // ========================================================================
    // Week 30 Phase 1: VK Registry types and codec
    // ========================================================================

    fn sample_vk_registration() -> VkRegistrationData {
        VkRegistrationData {
            version: VK_REGISTRATION_VERSION,
            // PROOF_TYPE_GROTH16 = 1 in crates/execution/src/lib.rs;
            // duplicated here as a literal because the discriminant
            // lives in the execution crate.
            proof_type: 1,
            code_hash: [0xC0u8; 32],
            label: b"sum-v1".to_vec(),
            vk_bytes: (0..16u8).collect(),
        }
    }

    #[test]
    fn vk_registration_constants_are_stable() {
        assert_eq!(MAX_VK_REGISTRATIONS_PER_ENTITY, 8);
        assert_eq!(VK_REGISTRATION_VERSION, 1);
        assert_eq!(VK_REGISTRATION_LABEL_MAX, 32);
        assert_eq!(VK_REGISTRATION_HEADER_SIZE, 39);
        assert_eq!(VK_REGISTRATION_HEADER_SIZE, 1 + 1 + 32 + 1 + 4);
    }

    #[test]
    fn vk_registration_roundtrip() {
        let reg = sample_vk_registration();
        let bytes = reg.encode();
        let decoded = VkRegistrationData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded, reg);
    }

    #[test]
    fn vk_registration_roundtrip_empty_label() {
        let mut reg = sample_vk_registration();
        reg.label = Vec::new();
        let bytes = reg.encode();
        assert_eq!(bytes[34], 0, "label_len byte must be 0 for empty label");
        let decoded = VkRegistrationData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded, reg);
    }

    #[test]
    fn vk_registration_roundtrip_max_label() {
        let mut reg = sample_vk_registration();
        reg.label = vec![0xABu8; VK_REGISTRATION_LABEL_MAX];
        let bytes = reg.encode();
        assert_eq!(bytes[34], VK_REGISTRATION_LABEL_MAX as u8);
        let decoded = VkRegistrationData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.label.len(), VK_REGISTRATION_LABEL_MAX);
        assert_eq!(decoded, reg);
    }

    #[test]
    fn vk_registration_decode_rejects_too_short() {
        // Anything shorter than the fixed header is rejected.
        for len in 0..VK_REGISTRATION_HEADER_SIZE {
            let bytes = vec![0u8; len];
            assert!(
                VkRegistrationData::decode(&bytes).is_none(),
                "len {len} must be rejected (below header)"
            );
        }
    }

    #[test]
    fn vk_registration_decode_rejects_trailing_bytes() {
        let reg = sample_vk_registration();
        let mut bytes = reg.encode();
        bytes.push(0xFF);
        assert!(VkRegistrationData::decode(&bytes).is_none());
    }

    #[test]
    fn vk_registration_decode_rejects_truncated_tail() {
        let reg = sample_vk_registration();
        let mut bytes = reg.encode();
        bytes.pop();
        assert!(VkRegistrationData::decode(&bytes).is_none());
    }

    #[test]
    fn vk_registration_decode_preserves_version_byte() {
        // Handler validates version; decoder does not. Byte 0 is preserved
        // verbatim so the runtime can surface a specific error variant on
        // version mismatch.
        let mut reg = sample_vk_registration();
        reg.version = 99;
        let bytes = reg.encode();
        let decoded = VkRegistrationData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.version, 99);
    }

    #[test]
    fn vk_registration_decode_preserves_proof_type_byte() {
        // Handler validates proof_type; decoder does not. Byte 1 is
        // preserved verbatim so the runtime can surface a specific
        // unsupported-proof-type error.
        let mut reg = sample_vk_registration();
        reg.proof_type = 99;
        let bytes = reg.encode();
        let decoded = VkRegistrationData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.proof_type, 99);
    }

    #[test]
    fn golden_vector_vk_registration_layout() {
        // Locks the byte layout for VK_REGISTRATION_VERSION = 1 against
        // accidental field reordering or width changes. The vk_bytes
        // portion is dummy content (not a real Groth16 VK); the codec is
        // byte-faithful and does not validate VK structure.
        let reg = sample_vk_registration();
        let bytes = reg.encode();

        let expected_len = VK_REGISTRATION_HEADER_SIZE + reg.label.len() + reg.vk_bytes.len();
        assert_eq!(bytes.len(), expected_len, "total length matches layout");

        assert_eq!(bytes[0], VK_REGISTRATION_VERSION, "version at 0");
        assert_eq!(bytes[1], 1, "proof_type (Groth16) at 1");
        assert_eq!(&bytes[2..34], &[0xC0u8; 32], "code_hash at 2..34");
        assert_eq!(bytes[34], 6u8, "label_len at 34");
        assert_eq!(&bytes[35..39], &16u32.to_be_bytes(), "vk_len_be at 35..39");
        assert_eq!(&bytes[39..45], b"sum-v1", "label at 39..45");
        let vk_expected: Vec<u8> = (0..16u8).collect();
        assert_eq!(&bytes[45..61], vk_expected.as_slice(), "vk_bytes at 45..61");
    }

    // ========================================================================
    // Week 31 Phase 1: SLA Agreement types and codec
    // ========================================================================

    fn sample_sla_agreement() -> SlaAgreementData {
        SlaAgreementData {
            version: SLA_AGREEMENT_V1,
            buyer_entity_id: [0x11u8; 32],
            seller_entity_id: [0x22u8; 32],
            service_descriptor_hash: [0x33u8; 32],
            status: SLA_STATUS_PROPOSED,
            created_at_height: 0x1234_5678_9ABC_DEF0,
            accepted_at_height: 0,
            start_height: 1_000,
            end_height: 5_000,
            violation_count: 0,
            violation_threshold: 3,
            max_response_time_blocks: 60,
            min_uptime_bps: 9_500,
            min_delivery_success_bps: 9_000,
            price_per_call: 0x0102_0304_0506_0708,
            slash_amount: 0x4142_4344_4546_4748_4A4B_4C4D_4E4F_5051,
            terminated_at_height: 0,
            slashed_amount: 0,
            reserved: [0u8; SLA_RESERVED_LEN],
        }
    }

    #[test]
    fn sla_agreement_constants_are_stable() {
        assert_eq!(MAX_SLAS_PER_ENTITY, 8);
        assert_eq!(SLA_AGREEMENT_V1, 1);
        assert_eq!(SLA_RESERVED_LEN, 16);
        assert_eq!(SLA_STATUS_PROPOSED, 0);
        assert_eq!(SLA_STATUS_ACTIVE, 1);
        assert_eq!(SLA_STATUS_COMPLETED, 2);
        assert_eq!(SLA_STATUS_VIOLATED, 3);
        assert_eq!(SLA_STATUS_CANCELLED, 4);
        assert_eq!(SLA_STATUS_MAX, SLA_STATUS_CANCELLED);
        assert_eq!(SLA_MIN_UPTIME_BPS_MAX, 10_000);
        assert_eq!(SLA_MIN_DELIVERY_SUCCESS_BPS_MAX, 10_000);
        assert_eq!(SLA_MAX_DURATION_BLOCKS, 604_800);
    }

    #[test]
    fn sla_agreement_size_constant_matches_layout() {
        assert_eq!(SLA_AGREEMENT_SIZE, 210);
        assert_eq!(
            SLA_AGREEMENT_SIZE,
            1 + 32 + 32 + 32 + 1 + 8 + 8 + 8 + 8 + 4 + 4 + 4 + 2 + 2 + 8 + 16 + 8 + 16 + 16
        );
    }

    #[test]
    fn sla_agreement_roundtrip() {
        let sla = sample_sla_agreement();
        let bytes = sla.encode();
        assert_eq!(bytes.len(), SLA_AGREEMENT_SIZE);
        let decoded = SlaAgreementData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded, sla);
    }

    #[test]
    fn sla_agreement_roundtrip_preserves_arbitrary_reserved_bytes() {
        // Decoder is byte-faithful: any non-zero `reserved` survives a
        // roundtrip. Handler-level validation rejects non-zero `reserved`
        // at create/update; the codec contract is to preserve bytes.
        let mut sla = sample_sla_agreement();
        sla.reserved = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
            0xFF, 0x10,
        ];
        let bytes = sla.encode();
        let decoded = SlaAgreementData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.reserved, sla.reserved);
    }

    #[test]
    fn sla_agreement_decode_rejects_wrong_length() {
        let sla = sample_sla_agreement();
        let mut bytes = sla.encode().to_vec();
        bytes.push(0);
        assert!(SlaAgreementData::decode(&bytes).is_none());
        bytes.pop();
        bytes.pop();
        assert!(SlaAgreementData::decode(&bytes).is_none());
    }

    #[test]
    fn sla_agreement_decode_preserves_version_byte() {
        // Handler validates version; decoder does not. Byte 0 is
        // preserved verbatim so the runtime can surface a specific
        // SlaAgreementVersionInvalid error on mismatch.
        let mut sla = sample_sla_agreement();
        sla.version = 99;
        let bytes = sla.encode();
        let decoded = SlaAgreementData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.version, 99);
    }

    #[test]
    fn sla_agreement_decode_preserves_status_byte() {
        // Handler validates the status discriminant; decoder does not.
        // Byte 97 is preserved verbatim so the runtime can surface a
        // specific SlaAgreementStatusInvalid error on out-of-range values.
        let mut sla = sample_sla_agreement();
        sla.status = 99;
        let bytes = sla.encode();
        let decoded = SlaAgreementData::decode(&bytes).expect("decode succeeds");
        assert_eq!(decoded.status, 99);
    }

    #[test]
    fn golden_vector_sla_agreement_210_bytes() {
        // Locks the byte layout for SLA_AGREEMENT_V1 = 1 against
        // accidental field reordering or width changes. Sample uses
        // distinctive byte values per field so a one-byte offset error
        // surfaces immediately.
        let sla = sample_sla_agreement();
        let bytes = sla.encode();
        assert_eq!(bytes.len(), 210);

        assert_eq!(bytes[0], SLA_AGREEMENT_V1, "version at 0");
        assert_eq!(&bytes[1..33], &[0x11u8; 32], "buyer_entity_id at 1..33");
        assert_eq!(&bytes[33..65], &[0x22u8; 32], "seller_entity_id at 33..65");
        assert_eq!(
            &bytes[65..97],
            &[0x33u8; 32],
            "service_descriptor_hash at 65..97"
        );
        assert_eq!(bytes[97], SLA_STATUS_PROPOSED, "status at 97");
        assert_eq!(
            &bytes[98..106],
            &0x1234_5678_9ABC_DEF0u64.to_be_bytes(),
            "created_at_height_be at 98..106"
        );
        assert_eq!(
            &bytes[106..114],
            &0u64.to_be_bytes(),
            "accepted_at_height_be at 106..114 (zero before acceptance)"
        );
        assert_eq!(
            &bytes[114..122],
            &1_000u64.to_be_bytes(),
            "start_height_be at 114..122"
        );
        assert_eq!(
            &bytes[122..130],
            &5_000u64.to_be_bytes(),
            "end_height_be at 122..130"
        );
        assert_eq!(
            &bytes[130..134],
            &0u32.to_be_bytes(),
            "violation_count_be at 130..134 (zero on create)"
        );
        assert_eq!(
            &bytes[134..138],
            &3u32.to_be_bytes(),
            "violation_threshold_be at 134..138"
        );
        assert_eq!(
            &bytes[138..142],
            &60u32.to_be_bytes(),
            "max_response_time_blocks_be at 138..142 (reserved v1)"
        );
        assert_eq!(
            &bytes[142..144],
            &9_500u16.to_be_bytes(),
            "min_uptime_bps_be at 142..144 (reserved v1)"
        );
        assert_eq!(
            &bytes[144..146],
            &9_000u16.to_be_bytes(),
            "min_delivery_success_bps_be at 144..146 (reserved v1)"
        );
        assert_eq!(
            &bytes[146..154],
            &0x0102_0304_0506_0708u64.to_be_bytes(),
            "price_per_call_be at 146..154"
        );
        assert_eq!(
            &bytes[154..170],
            &0x4142_4344_4546_4748_4A4B_4C4D_4E4F_5051u128.to_be_bytes(),
            "slash_amount_be at 154..170"
        );
        assert_eq!(
            &bytes[170..178],
            &0u64.to_be_bytes(),
            "terminated_at_height_be at 170..178 (zero on create)"
        );
        assert_eq!(
            &bytes[178..194],
            &0u128.to_be_bytes(),
            "slashed_amount_be at 178..194 (zero on create)"
        );
        assert_eq!(&bytes[194..210], &[0u8; 16], "reserved at 194..210 (zero)");
    }
}
