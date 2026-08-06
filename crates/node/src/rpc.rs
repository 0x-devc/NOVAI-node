//! JSON-RPC 2.0 server for transaction submission and state queries.
//!
//! PURPOSE: Provide HTTP JSON-RPC endpoint for external clients to:
//! - Submit transactions to mempool
//! - Query signal commitments (Week 14 - D14.5)
//!
//! INVARIANTS:
//! - Server binds to specified address on startup
//! - Accepts only JSON-RPC 2.0 requests
//! - Returns txid on successful submission to mempool
//! - Signal queries return deterministic results
//!
//! FAILURE MODES:
//! - Port already in use → returns error on start
//! - Invalid transaction → returns RPC error -32000
//! - Mempool full → returns RPC error -32001
//! - State query error → returns RPC error -32002

use crate::consensus_node::Storage;
use crate::faucet_rate_limit::FaucetRateLimit;
use crate::MutexExt;
use mempool::{NonceProvider, TxMempool};
use novai_ai_entities::{
    AiSignalType, MemoryObject, PaymentChannelData, ServiceDescriptorData, SignalCommitment,
    SlaAgreementData, VkRegistrationData, PAYMENT_CHANNEL_STATUS_CLOSING,
    PAYMENT_CHANNEL_STATUS_OPEN, PAYMENT_CHANNEL_STATUS_PROPOSED, SERVICE_CATEGORY_COMPUTE,
    SERVICE_CATEGORY_DATA_ORACLE, SERVICE_CATEGORY_GATEWAY, SERVICE_CATEGORY_GENERIC,
    SERVICE_CATEGORY_INDEXER, SERVICE_CATEGORY_INFERENCE, SERVICE_CATEGORY_MONITORING,
    SERVICE_CATEGORY_RESERVED_MAX, SERVICE_CATEGORY_SIGNAL_PROVIDER, SERVICE_CATEGORY_STORAGE,
    SERVICE_CATEGORY_VERIFICATION, SERVICE_STATUS_ACTIVE, SERVICE_STATUS_DEPRECATED,
    SERVICE_STATUS_PAUSED, SLA_STATUS_ACTIVE, SLA_STATUS_CANCELLED, SLA_STATUS_COMPLETED,
    SLA_STATUS_PROPOSED, SLA_STATUS_VIOLATED,
};
use novai_codec::{decode_tx_v1_signed, txid_v1};
use novai_consensus_types;
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_execution::{
    get_active_sla_between, get_channels_by_party_a, get_channels_by_party_b,
    get_memory_objects_by_entity, get_oracle_anchor, get_oracle_anchors_by_entity,
    get_oracle_anchors_by_tag, get_payment_channel,
    get_payments_with_splits_and_condition_by_entity, get_service_descriptors_by_category,
    get_signals_by_height, get_signals_by_issuer, get_signals_by_type, get_sla_agreement,
    get_slas_by_buyer, get_slas_by_seller, get_upgrade_history, get_vk_registration_by_id,
    get_vk_registrations_by_entity, read_account_or_default, read_ai_entity, read_upgrade_summary,
    OracleAnchorRecord, PaymentCondition, PaymentRecord, PaymentRole, PaymentSplitsRecord,
    PaymentSplitsRecordEntry, UpgradeRecord, ORACLE_ANCHOR_DATA_TAG_MAX_LEN,
    PAYMENT_ATTESTATION_STATUS_DELIVERED, PAYMENT_ATTESTATION_STATUS_FAILED,
    PAYMENT_ATTESTATION_STATUS_NONE, PROOF_TYPE_GROTH16, PROOF_TYPE_GROTH16_REGISTERED,
    PROOF_TYPE_PLONK, PROOF_TYPE_PLONK_REGISTERED, PROOF_TYPE_STUB,
};
use novai_p2p::{NetworkMessage, PeerManager};
use novai_types::{Address, TxV1, TxVersion};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Response, Server, StatusCode};

/// Maximum RPC requests per second before rate limiting kicks in.
const MAX_RPC_REQUESTS_PER_SEC: usize = 100;

/// Maximum RPC request body size in bytes.
/// Prevents OOM from oversized HTTP bodies. 512KB is generous for any
/// valid JSON-RPC request (max tx is 128KB → 256KB hex-encoded).
const MAX_RPC_BODY_SIZE: usize = 512 * 1024;

/// Maximum concurrent RPC requests being processed (C-06).
/// Prevents thread exhaustion from Slowloris or SYN flood attacks.
const MAX_CONCURRENT_RPC: usize = 64;

/// Maximum height range for signal queries (prevents massive result sets).
const MAX_SIGNAL_QUERY_RANGE: u64 = 10_000;

/// Sized wrapper around a shared `NonceProvider` trait object.
///
/// Needed because `TxMempool::insert` takes `&impl NonceProvider` (requires
/// `Sized`), but the RPC server holds an `Arc<dyn NonceProvider>`.
struct SharedNonceProvider(Arc<dyn NonceProvider + Send + Sync>);

impl NonceProvider for SharedNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        self.0.expected_nonce(from)
    }
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    method: String,
    params: serde_json::Value,
    id: serde_json::Value,
}

/// JSON-RPC 2.0 success response.
#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    result: serde_json::Value,
    id: serde_json::Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Serialize)]
struct RpcErrorResponse {
    jsonrpc: &'static str,
    error: RpcError,
    id: serde_json::Value,
}

/// JSON-RPC error object.
#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Parameters for novai_submitTransaction.
#[derive(Debug, Deserialize)]
struct SubmitTxParams {
    tx: String, // Hex-encoded signed transaction
}

/// Result for novai_submitTransaction.
#[derive(Debug, Serialize)]
struct SubmitTxResult {
    txid: String, // Hex-encoded transaction ID
}

/// Parameters for novai_getNonce.
#[derive(Debug, Deserialize)]
struct GetNonceParams {
    address: String, // Hex-encoded 32-byte address
}

/// Result for novai_getNonce.
#[derive(Debug, Serialize)]
struct GetNonceResult {
    nonce: u64,
}

// ============================================================================
// BLOCK EXPLORER RPC TYPES (P1-4 + P1-5)
// ============================================================================

/// Shared index populated during block commits.
/// Provides O(1) tx receipt and block hash lookups for the RPC layer.
#[derive(Default)]
pub struct BlockchainIndex {
    /// txid → (block_height, tx_index)
    pub tx_receipts: HashMap<[u8; 32], (u64, usize)>,
    /// block_hash → block_height
    pub block_hashes: HashMap<[u8; 32], u64>,
    /// Latest committed height
    pub committed_height: u64,
}

impl BlockchainIndex {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Parameters for novai_getTransaction.
#[derive(Debug, Deserialize)]
struct GetTxParams {
    txid: String,
}

/// Result for novai_getTransaction.
#[derive(Debug, Serialize)]
struct GetTxResult {
    block_height: u64,
    tx_index: usize,
    from: String,
    nonce: u64,
    fee: u64,
    payload_len: usize,
}

/// Parameters for novai_getBlockByHeight.
#[derive(Debug, Deserialize)]
struct GetBlockByHeightParams {
    height: u64,
}

/// Parameters for novai_getBlockByHash.
#[derive(Debug, Deserialize)]
struct GetBlockByHashParams {
    hash: String,
}

/// Result for block queries.
#[derive(Debug, Serialize)]
struct BlockResult {
    height: u64,
    round: u64,
    block_hash: String,
    parent_hash: String,
    state_root: String,
    tx_count: usize,
}

// ============================================================================
// SIGNAL QUERY RPC TYPES (Week 14 - D14.5)
// ============================================================================

/// Parameters for novai_getSignalsByHeight.
#[derive(Debug, Deserialize)]
struct GetSignalsByHeightParams {
    height: u64,
}

/// Parameters for novai_getSignalsByIssuer.
#[derive(Debug, Deserialize)]
struct GetSignalsByIssuerParams {
    issuer: String, // Hex-encoded 32-byte issuer entity ID
    start_height: u64,
    end_height: u64,
}

/// Parameters for novai_getSignalsByType.
#[derive(Debug, Deserialize)]
struct GetSignalsByTypeParams {
    signal_type: u8, // 0-6
    start_height: u64,
    end_height: u64,
}

/// JSON-serializable signal commitment.
#[derive(Debug, Serialize)]
struct SignalCommitmentJson {
    commitment_hash: String, // Hex-encoded
    signal_type: u8,
    height: u64,
    issuer: String, // Hex-encoded
}

impl From<SignalCommitment> for SignalCommitmentJson {
    fn from(c: SignalCommitment) -> Self {
        Self {
            commitment_hash: hex::encode(c.commitment_hash),
            signal_type: c.signal_type.to_byte(),
            height: c.height,
            issuer: hex::encode(c.issuer),
        }
    }
}

/// Result for signal query methods.
#[derive(Debug, Serialize)]
struct SignalQueryResult {
    signals: Vec<SignalCommitmentJson>,
}

/// Parameters for novai_getPaymentsByEntity.
#[derive(Debug, Deserialize)]
struct GetPaymentsByEntityParams {
    /// Hex-encoded 32-byte entity ID. Whose payments to return.
    entity_id: String,
    /// Either "payer" (outgoing payments) or "payee" (incoming).
    role: String,
    /// Inclusive lower bound on `payment_height`.
    start_height: u64,
    /// Inclusive upper bound on `payment_height`. Must not exceed
    /// `start_height + MAX_SIGNAL_QUERY_RANGE`.
    end_height: u64,
}

/// JSON-serializable single split entry attached to a multi-party
/// payment (Week 33). `recipient_entity_id` is hex; `credited_amount`
/// is a decimal string consistent with `PaymentJson.amount`.
#[derive(Debug, Serialize)]
struct PaymentSplitJson {
    /// Hex-encoded 32-byte recipient entity id.
    recipient_entity_id: String,
    /// Basis points share of the payment's `amount` for this recipient.
    basis_points: u16,
    /// Amount actually credited to this recipient in base units. For
    /// `splits[0]` this includes the floor-division remainder folded
    /// in by the executor so the sum of `credited_amount` across all
    /// entries equals `PaymentJson.amount` exactly.
    credited_amount: String,
}

impl From<PaymentSplitsRecordEntry> for PaymentSplitJson {
    fn from(e: PaymentSplitsRecordEntry) -> Self {
        Self {
            recipient_entity_id: hex::encode(e.recipient_entity_id),
            basis_points: e.basis_points,
            credited_amount: e.credited_amount.to_string(),
        }
    }
}

/// JSON-serializable payment condition (Week 36, conditional execution).
/// `kind` is a stable snake_case label and `anchor_signal_hash` is hex. The
/// kind-specific operands are present only for the kinds that carry them
/// (`expected_data_hash` for `anchor_data_hash_equals`; `expected_tag` /
/// `expected_tag_hex` for `anchor_tag_equals`) and `null` otherwise.
#[derive(Debug, Serialize)]
struct PaymentConditionJson {
    /// Condition kind: `"anchor_exists"`, `"anchor_data_hash_equals"`,
    /// `"anchor_tag_equals"`, or `"anchor_not_expired"`.
    kind: &'static str,
    /// Hex-encoded 32-byte `signal_hash` of the referenced oracle anchor.
    anchor_signal_hash: String,
    /// Required `data_hash` (hex) for `anchor_data_hash_equals`; `null`
    /// for other kinds.
    expected_data_hash: Option<String>,
    /// Required `data_tag` (lossy UTF-8) for `anchor_tag_equals`; `null`
    /// for other kinds.
    expected_tag: Option<String>,
    /// Required `data_tag` (exact bytes, hex) for `anchor_tag_equals`;
    /// `null` for other kinds.
    expected_tag_hex: Option<String>,
}

impl From<PaymentCondition> for PaymentConditionJson {
    fn from(c: PaymentCondition) -> Self {
        match c {
            PaymentCondition::AnchorExists { anchor_signal_hash } => Self {
                kind: "anchor_exists",
                anchor_signal_hash: hex::encode(anchor_signal_hash),
                expected_data_hash: None,
                expected_tag: None,
                expected_tag_hex: None,
            },
            PaymentCondition::AnchorDataHashEquals {
                anchor_signal_hash,
                expected_data_hash,
            } => Self {
                kind: "anchor_data_hash_equals",
                anchor_signal_hash: hex::encode(anchor_signal_hash),
                expected_data_hash: Some(hex::encode(expected_data_hash)),
                expected_tag: None,
                expected_tag_hex: None,
            },
            PaymentCondition::AnchorTagEquals {
                anchor_signal_hash,
                expected_tag,
            } => Self {
                kind: "anchor_tag_equals",
                anchor_signal_hash: hex::encode(anchor_signal_hash),
                expected_data_hash: None,
                expected_tag: Some(String::from_utf8_lossy(&expected_tag).into_owned()),
                expected_tag_hex: Some(hex::encode(&expected_tag)),
            },
            PaymentCondition::AnchorNotExpired { anchor_signal_hash } => Self {
                kind: "anchor_not_expired",
                anchor_signal_hash: hex::encode(anchor_signal_hash),
                expected_data_hash: None,
                expected_tag: None,
                expected_tag_hex: None,
            },
        }
    }
}

/// JSON-serializable PaymentRecord. Amount is rendered as a decimal
/// string (consistent with the rest of the RPC surface where u64/u128
/// values are returned as strings to avoid JSON precision loss);
/// attested_status is rendered as a human-readable label. Week 33
/// adds an optional `splits` array surfaced from the
/// `PaymentSplitsRecord` aux row when present.
#[derive(Debug, Serialize)]
struct PaymentJson {
    /// Hex-encoded 32-byte payer entity id.
    payer: String,
    /// Hex-encoded 32-byte payee entity id. For multi-party payments
    /// this is the primary recipient (== `splits[0].recipient_entity_id`).
    payee: String,
    /// Payment amount in base units, as a decimal string.
    amount: String,
    /// Hex-encoded 32-byte service identifier carried verbatim from the
    /// `PaymentRequest` tail.
    service_descriptor_hash: String,
    /// Hex-encoded 32-byte per-request commitment.
    request_hash: String,
    /// Block height at which the payment settled.
    payment_height: u64,
    /// Absolute block height past which the payment would have been
    /// rejected as expired.
    max_block_height: u64,
    /// Attestation status: `null` until a matching `ServiceAttestation`
    /// is processed; otherwise `"delivered"` or `"failed"`.
    attested_status: Option<&'static str>,
    /// Block height at which the attestation was recorded; `null` when
    /// no attestation has been recorded yet.
    attested_height: Option<u64>,
    /// Week 33: per-recipient split breakdown for multi-party payments.
    /// `null` for legacy single-recipient payments (no aux record in
    /// storage); otherwise a length-2-to-8 array whose
    /// `credited_amount` values sum to `amount`.
    splits: Option<Vec<PaymentSplitJson>>,
    /// Week 36: the oracle-anchor condition that gated this payment.
    /// `null` for unconditional payments (no aux record); otherwise the
    /// condition reconstructed from the `PaymentConditionRecord`.
    condition: Option<PaymentConditionJson>,
}

impl From<PaymentRecord> for PaymentJson {
    fn from(r: PaymentRecord) -> Self {
        let (attested_status, attested_height) = match r.attested_status {
            PAYMENT_ATTESTATION_STATUS_NONE => (None, None),
            PAYMENT_ATTESTATION_STATUS_DELIVERED => (Some("delivered"), Some(r.attested_height)),
            PAYMENT_ATTESTATION_STATUS_FAILED => (Some("failed"), Some(r.attested_height)),
            // Unknown status bytes are impossible under the handler's
            // validation, but if a corrupted record somehow lands in
            // state we surface "unknown" rather than panicking.
            _ => (Some("unknown"), Some(r.attested_height)),
        };
        Self {
            payer: hex::encode(r.payer),
            payee: hex::encode(r.payee),
            amount: r.amount.to_string(),
            service_descriptor_hash: hex::encode(r.service_descriptor_hash),
            request_hash: hex::encode(r.request_hash),
            payment_height: r.payment_height,
            max_block_height: r.max_block_height,
            attested_status,
            attested_height,
            splits: None,
            condition: None,
        }
    }
}

impl PaymentJson {
    /// Build a `PaymentJson` from a `PaymentRecord` plus the optional
    /// Week 33 splits aux row. When `splits` is `Some`, the returned
    /// JSON carries a populated `splits` array; when `None`, the field
    /// serialises as `null` (preserving the Week 28 wire shape for
    /// legacy single-recipient payments).
    fn from_record_with_splits(record: PaymentRecord, splits: Option<PaymentSplitsRecord>) -> Self {
        let mut json = Self::from(record);
        if let Some(s) = splits {
            json.splits = Some(s.entries.into_iter().map(PaymentSplitJson::from).collect());
        }
        json
    }

    /// Build a `PaymentJson` from a `PaymentRecord` plus the optional
    /// Week 33 splits aux row and the optional Week 36 condition aux row.
    /// `splits` / `condition` serialise as `null` when absent, preserving
    /// the Week 28/33 wire shapes for existing consumers.
    fn from_record_with_splits_and_condition(
        record: PaymentRecord,
        splits: Option<PaymentSplitsRecord>,
        condition: Option<PaymentCondition>,
    ) -> Self {
        let mut json = Self::from_record_with_splits(record, splits);
        json.condition = condition.map(PaymentConditionJson::from);
        json
    }
}

/// Result for novai_getPaymentsByEntity.
#[derive(Debug, Serialize)]
struct GetPaymentsByEntityResult {
    payments: Vec<PaymentJson>,
}

/// Parameters for novai_getServiceDescriptorsByCategory (Week 29).
#[derive(Debug, Deserialize)]
struct GetServiceDescriptorsByCategoryParams {
    /// Service category discriminant (0..=15 well-known; 16..=255 reserved
    /// for governance allocation).
    category: u8,
}

/// JSON-serializable `ServiceDescriptor` record (Week 29).
///
/// `price_per_call`, `subscription_rate_per_block`, and `min_stake` are
/// rendered as decimal strings to avoid JSON precision loss on large
/// u64 / u128 values (matching the rest of the RPC surface).
/// `category` and `status` carry both their numeric byte and a
/// human-readable label so clients do not need a side-channel mapping.
#[derive(Debug, Serialize)]
struct ServiceDescriptorJson {
    /// Hex-encoded 32-byte memory object id.
    object_id: String,
    /// Hex-encoded 32-byte publisher entity id.
    owner_entity: String,
    /// Block height at which the descriptor was first published.
    created_at: u64,
    /// Block height at which the descriptor was most recently updated.
    /// Equals `created_at` if no update has landed.
    updated_at: u64,
    /// Wire-format version byte.
    version: u8,
    /// Hex-encoded 32-byte off-chain canonical service name commitment.
    service_name_hash: String,
    /// Hex-encoded 32-byte off-chain endpoint URL commitment.
    service_url_hash: String,
    /// Hex-encoded 32-byte off-chain long description commitment.
    description_hash: String,
    /// Numeric category discriminant.
    category: u8,
    /// Human-readable category label. Well-known values render their
    /// canonical name (`"data-oracle"`, `"inference"`, ...); reserved
    /// values render as `"reserved"`; governance-allocated values
    /// (currently impossible) would render as `"governance"`.
    category_label: &'static str,
    /// Per-call price in base units, as a decimal string. `"0"` means
    /// the service is free.
    price_per_call: String,
    /// Per-block subscription rate as a decimal string. `"0"` means no
    /// subscription pricing is offered.
    subscription_rate_per_block: String,
    /// Minimum caller reputation score.
    min_reputation_score: u16,
    /// Minimum caller stake balance, as a decimal string.
    min_stake: String,
    /// Capability-tag bitfield (u32; bit semantics defined off-chain).
    capability_tags: u32,
    /// Numeric status discriminant.
    status: u8,
    /// Human-readable status label (`"active"`, `"paused"`,
    /// `"deprecated"`). Unknown status bytes render as `"unknown"`
    /// rather than panicking, mirroring the `attested_status` fallback
    /// on `PaymentJson`.
    status_label: &'static str,
}

impl From<(MemoryObject, ServiceDescriptorData)> for ServiceDescriptorJson {
    fn from(pair: (MemoryObject, ServiceDescriptorData)) -> Self {
        let (obj, sd) = pair;
        Self {
            object_id: hex::encode(obj.object_id),
            owner_entity: hex::encode(obj.owner_entity),
            created_at: obj.created_at,
            updated_at: obj.updated_at,
            version: sd.version,
            service_name_hash: hex::encode(sd.service_name_hash),
            service_url_hash: hex::encode(sd.service_url_hash),
            description_hash: hex::encode(sd.description_hash),
            category: sd.category,
            category_label: service_category_label(sd.category),
            price_per_call: sd.price_per_call.to_string(),
            subscription_rate_per_block: sd.subscription_rate_per_block.to_string(),
            min_reputation_score: sd.min_reputation_score,
            min_stake: sd.min_stake.to_string(),
            capability_tags: sd.capability_tags,
            status: sd.status,
            status_label: service_status_label(sd.status),
        }
    }
}

fn service_category_label(byte: u8) -> &'static str {
    match byte {
        SERVICE_CATEGORY_GENERIC => "generic",
        SERVICE_CATEGORY_DATA_ORACLE => "data-oracle",
        SERVICE_CATEGORY_INFERENCE => "inference",
        SERVICE_CATEGORY_COMPUTE => "compute",
        SERVICE_CATEGORY_STORAGE => "storage",
        SERVICE_CATEGORY_INDEXER => "indexer",
        SERVICE_CATEGORY_SIGNAL_PROVIDER => "signal-provider",
        SERVICE_CATEGORY_VERIFICATION => "verification",
        SERVICE_CATEGORY_MONITORING => "monitoring",
        SERVICE_CATEGORY_GATEWAY => "gateway",
        b if b <= SERVICE_CATEGORY_RESERVED_MAX => "reserved",
        _ => "governance",
    }
}

fn service_status_label(byte: u8) -> &'static str {
    match byte {
        SERVICE_STATUS_ACTIVE => "active",
        SERVICE_STATUS_PAUSED => "paused",
        SERVICE_STATUS_DEPRECATED => "deprecated",
        _ => "unknown",
    }
}

/// Result for novai_getServiceDescriptorsByCategory.
#[derive(Debug, Serialize)]
struct GetServiceDescriptorsByCategoryResult {
    descriptors: Vec<ServiceDescriptorJson>,
}

/// Parameters for novai_getVkRegistration (Week 30).
#[derive(Debug, Deserialize)]
struct GetVkRegistrationParams {
    /// Hex-encoded 32-byte VK registry handle (the memory object id
    /// of a published `VkRegistration`).
    id: String,
}

/// Parameters for novai_listVkRegistrations (Week 30).
#[derive(Debug, Deserialize)]
struct ListVkRegistrationsParams {
    /// Hex-encoded 32-byte entity id whose VK registrations to list.
    entity_id: String,
}

/// JSON-serializable `VkRegistration` record (Week 30).
///
/// `vk_bytes_hex` carries the full compressed VK so clients have a
/// complete record without a follow-up fetch. `label` is rendered as a
/// raw UTF-8 string (lossy on non-UTF-8 — handler validation forces
/// labels through the same 32-byte / UTF-8-ish cap, so lossy decoding
/// is acceptable for display).
#[derive(Debug, Serialize)]
struct VkRegistrationJson {
    /// Hex-encoded 32-byte memory object id (the canonical registry
    /// handle a `ProofSubmission` carries in `vk_bytes` when
    /// `proof_type == PROOF_TYPE_GROTH16_REGISTERED`).
    object_id: String,
    /// Hex-encoded 32-byte owner entity id.
    owner_entity: String,
    /// Block height at which the registration was first published.
    created_at: u64,
    /// Block height at which the registration was most recently updated.
    /// Equals `created_at` if no update has landed. Only `label` may
    /// change; the proof_type / code_hash / vk_bytes are immutable.
    updated_at: u64,
    /// Wire-format version byte.
    version: u8,
    /// Numeric proof-system discriminant.
    proof_type: u8,
    /// Human-readable proof-system label (`"groth16"`, `"stub"`,
    /// `"plonk"`, ...). Unknown discriminants render as `"unknown"`.
    proof_type_label: &'static str,
    /// Hex-encoded 32-byte canonical `code_hash` the VK verifies.
    /// `ProofSubmission` callers must match this exactly.
    code_hash: String,
    /// Free-form label as a lossily-decoded UTF-8 string.
    label: String,
    /// Length of the compressed VK in bytes (convenience field — equals
    /// `hex::decode(vk_bytes_hex).len()`).
    vk_len: usize,
    /// Hex-encoded compressed verification key bytes.
    vk_bytes_hex: String,
}

impl From<(MemoryObject, VkRegistrationData)> for VkRegistrationJson {
    fn from(pair: (MemoryObject, VkRegistrationData)) -> Self {
        let (obj, reg) = pair;
        let label = String::from_utf8_lossy(&reg.label).into_owned();
        Self {
            object_id: hex::encode(obj.object_id),
            owner_entity: hex::encode(obj.owner_entity),
            created_at: obj.created_at,
            updated_at: obj.updated_at,
            version: reg.version,
            proof_type: reg.proof_type,
            proof_type_label: proof_type_label(reg.proof_type),
            code_hash: hex::encode(reg.code_hash),
            label,
            vk_len: reg.vk_bytes.len(),
            vk_bytes_hex: hex::encode(&reg.vk_bytes),
        }
    }
}

fn proof_type_label(byte: u8) -> &'static str {
    match byte {
        PROOF_TYPE_STUB => "stub",
        PROOF_TYPE_GROTH16 => "groth16",
        PROOF_TYPE_PLONK => "plonk",
        PROOF_TYPE_GROTH16_REGISTERED => "groth16-registered",
        PROOF_TYPE_PLONK_REGISTERED => "plonk-registered",
        _ => "unknown",
    }
}

/// Result for novai_getVkRegistration.
#[derive(Debug, Serialize)]
struct GetVkRegistrationResult {
    /// `None` if no registration matches the supplied handle (either
    /// it never existed or it was deleted).
    registration: Option<VkRegistrationJson>,
}

/// Result for novai_listVkRegistrations.
#[derive(Debug, Serialize)]
struct ListVkRegistrationsResult {
    registrations: Vec<VkRegistrationJson>,
}

/// Parameters for novai_getSlaAgreement (Week 31).
#[derive(Debug, Deserialize)]
struct GetSlaAgreementParams {
    /// Hex-encoded 32-byte buyer entity id (the SLA's memory-object owner).
    owner: String,
    /// Hex-encoded 32-byte SLA memory object id.
    object_id: String,
}

/// Parameters for novai_getActiveSla (Week 31).
#[derive(Debug, Deserialize)]
struct GetActiveSlaParams {
    /// Hex-encoded 32-byte buyer entity id.
    buyer: String,
    /// Hex-encoded 32-byte seller entity id.
    seller: String,
}

/// Parameters for novai_listSlasByBuyer / novai_listSlasBySeller (Week 31).
#[derive(Debug, Deserialize)]
struct ListSlasParams {
    /// Hex-encoded 32-byte entity id (buyer or seller, depending on the
    /// RPC method).
    entity_id: String,
    /// Inclusive lower bound on the SLA memory object's
    /// `created_at` height.
    start_height: u64,
    /// Inclusive upper bound on the SLA memory object's
    /// `created_at` height.
    end_height: u64,
}

/// JSON-serializable `SlaAgreement` record (Week 31).
///
/// Decimal-string encoding is used for the `u128` `slash_amount` and
/// `slashed_amount` fields to avoid JSON precision loss, matching the
/// convention shipped in Week 28 for `PaymentRecord.amount` and
/// Week 29 for `ServiceDescriptorData.min_stake`. Clients compute
/// `is_expired` locally by comparing `end_height` against the chain's
/// committed head (returned by `novai_getStatus`).
#[derive(Debug, Serialize)]
struct SlaAgreementJson {
    object_id: String,
    owner_entity: String,
    created_at: u64,
    updated_at: u64,
    version: u8,
    buyer_entity_id: String,
    seller_entity_id: String,
    service_descriptor_hash: String,
    status: u8,
    /// `"proposed" | "active" | "completed" | "violated" | "cancelled"`
    /// for the well-known v1 discriminants; out-of-range bytes render
    /// as `"unknown"` rather than panicking.
    status_label: &'static str,
    created_at_height: u64,
    accepted_at_height: u64,
    start_height: u64,
    end_height: u64,
    violation_count: u32,
    violation_threshold: u32,
    max_response_time_blocks: u32,
    min_uptime_bps: u16,
    min_delivery_success_bps: u16,
    /// Per-call price as a decimal string.
    price_per_call: String,
    /// Penalty paid on threshold breach, as a decimal string.
    slash_amount: String,
    terminated_at_height: u64,
    /// Actual debit applied on auto-slash, as a decimal string.
    slashed_amount: String,
}

impl From<(MemoryObject, SlaAgreementData)> for SlaAgreementJson {
    fn from(pair: (MemoryObject, SlaAgreementData)) -> Self {
        let (obj, sla) = pair;
        Self {
            object_id: hex::encode(obj.object_id),
            owner_entity: hex::encode(obj.owner_entity),
            created_at: obj.created_at,
            updated_at: obj.updated_at,
            version: sla.version,
            buyer_entity_id: hex::encode(sla.buyer_entity_id),
            seller_entity_id: hex::encode(sla.seller_entity_id),
            service_descriptor_hash: hex::encode(sla.service_descriptor_hash),
            status: sla.status,
            status_label: sla_status_label(sla.status),
            created_at_height: sla.created_at_height,
            accepted_at_height: sla.accepted_at_height,
            start_height: sla.start_height,
            end_height: sla.end_height,
            violation_count: sla.violation_count,
            violation_threshold: sla.violation_threshold,
            max_response_time_blocks: sla.max_response_time_blocks,
            min_uptime_bps: sla.min_uptime_bps,
            min_delivery_success_bps: sla.min_delivery_success_bps,
            price_per_call: sla.price_per_call.to_string(),
            slash_amount: sla.slash_amount.to_string(),
            terminated_at_height: sla.terminated_at_height,
            slashed_amount: sla.slashed_amount.to_string(),
        }
    }
}

fn sla_status_label(byte: u8) -> &'static str {
    match byte {
        SLA_STATUS_PROPOSED => "proposed",
        SLA_STATUS_ACTIVE => "active",
        SLA_STATUS_COMPLETED => "completed",
        SLA_STATUS_VIOLATED => "violated",
        SLA_STATUS_CANCELLED => "cancelled",
        _ => "unknown",
    }
}

/// Result for novai_getSlaAgreement / novai_getActiveSla.
#[derive(Debug, Serialize)]
struct GetSlaAgreementResult {
    agreement: Option<SlaAgreementJson>,
}

/// Result for novai_listSlasByBuyer / novai_listSlasBySeller.
#[derive(Debug, Serialize)]
struct ListSlasResult {
    agreements: Vec<SlaAgreementJson>,
}

/// Parameters for novai_getPaymentChannel and
/// novai_getChannelDisputeStatus (Week 32).
#[derive(Debug, Deserialize)]
struct GetPaymentChannelParams {
    /// Hex-encoded 32-byte party A entity id (the channel's
    /// memory-object owner).
    owner: String,
    /// Hex-encoded 32-byte channel memory object id.
    object_id: String,
}

/// Parameters for novai_listChannelsByPartyA /
/// novai_listChannelsByPartyB (Week 32).
#[derive(Debug, Deserialize)]
struct ListChannelsParams {
    /// Hex-encoded 32-byte entity id (party A or party B depending
    /// on the RPC method).
    entity_id: String,
    /// Inclusive lower bound on the channel memory object's
    /// `created_at` height (its `proposed_at_height`).
    start_height: u64,
    /// Inclusive upper bound on the channel memory object's
    /// `created_at` height.
    end_height: u64,
}

/// JSON-serializable `PaymentChannel` record (Week 32).
///
/// Decimal-string encoding is used for the `u128` deposit and
/// balance fields to avoid JSON precision loss, matching Week 28
/// `PaymentRecord.amount`, Week 29 `ServiceDescriptorData.min_stake`,
/// and Week 31 `SlaAgreementData.slash_amount`.
#[derive(Debug, Serialize)]
struct PaymentChannelJson {
    object_id: String,
    owner_entity: String,
    created_at: u64,
    updated_at: u64,
    version: u8,
    party_a_entity_id: String,
    party_b_entity_id: String,
    sla_object_id: String,
    status: u8,
    /// `"proposed" | "open" | "closing"` for the well-known v1
    /// discriminants; out-of-range bytes render as `"unknown"`.
    status_label: &'static str,
    /// Party A's deposit as a decimal string.
    deposit_a: String,
    /// Party B's deposit as a decimal string.
    deposit_b: String,
    /// Party A's currently recorded balance as a decimal string.
    balance_a: String,
    /// Party B's currently recorded balance as a decimal string.
    balance_b: String,
    /// Highest applied off-chain state nonce.
    nonce: u64,
    proposed_at_height: u64,
    accepted_at_height: u64,
    closing_at_height: u64,
    dispute_deadline_height: u64,
    dispute_window_blocks: u32,
}

impl From<(MemoryObject, PaymentChannelData)> for PaymentChannelJson {
    fn from(pair: (MemoryObject, PaymentChannelData)) -> Self {
        let (obj, channel) = pair;
        Self {
            object_id: hex::encode(obj.object_id),
            owner_entity: hex::encode(obj.owner_entity),
            created_at: obj.created_at,
            updated_at: obj.updated_at,
            version: channel.version,
            party_a_entity_id: hex::encode(channel.party_a_entity_id),
            party_b_entity_id: hex::encode(channel.party_b_entity_id),
            sla_object_id: hex::encode(channel.sla_object_id),
            status: channel.status,
            status_label: payment_channel_status_label(channel.status),
            deposit_a: channel.deposit_a.to_string(),
            deposit_b: channel.deposit_b.to_string(),
            balance_a: channel.balance_a.to_string(),
            balance_b: channel.balance_b.to_string(),
            nonce: channel.nonce,
            proposed_at_height: channel.proposed_at_height,
            accepted_at_height: channel.accepted_at_height,
            closing_at_height: channel.closing_at_height,
            dispute_deadline_height: channel.dispute_deadline_height,
            dispute_window_blocks: channel.dispute_window_blocks,
        }
    }
}

fn payment_channel_status_label(byte: u8) -> &'static str {
    match byte {
        PAYMENT_CHANNEL_STATUS_PROPOSED => "proposed",
        PAYMENT_CHANNEL_STATUS_OPEN => "open",
        PAYMENT_CHANNEL_STATUS_CLOSING => "closing",
        _ => "unknown",
    }
}

/// Result for novai_getPaymentChannel.
#[derive(Debug, Serialize)]
struct GetPaymentChannelResult {
    channel: Option<PaymentChannelJson>,
}

/// Result for novai_listChannelsByPartyA / novai_listChannelsByPartyB.
#[derive(Debug, Serialize)]
struct ListChannelsResult {
    channels: Vec<PaymentChannelJson>,
}

/// Result for novai_getChannelDisputeStatus (Week 32).
///
/// Returns dispute-window-relevant fields plus a derived
/// `blocks_remaining` so clients do not have to combine a separate
/// `novai_getStatus` call with the channel record. `found = false`
/// when the channel does not exist or is not a `PaymentChannel`;
/// the other fields are then all zero.
#[derive(Debug, Serialize)]
struct GetChannelDisputeStatusResult {
    /// `true` when the channel resolved and decoded; `false`
    /// otherwise (in which case the other fields are placeholder
    /// zeros).
    found: bool,
    status: u8,
    status_label: &'static str,
    closing_at_height: u64,
    dispute_deadline_height: u64,
    current_height: u64,
    /// `dispute_deadline_height.saturating_sub(current_height)`. Zero
    /// if the deadline has passed (the channel is ready to be
    /// finalized) OR if the channel is not in the CLOSING state.
    blocks_remaining: u64,
    /// `true` iff `status == CLOSING && current_height >
    /// dispute_deadline_height`. A finalize signal submitted at the
    /// current height would succeed (modulo per-tx gates).
    finalize_ready: bool,
}

// ============================================================================
// STATE QUERY RPC TYPES (CLI support)
// ============================================================================

/// Parameters for novai_getBalance.
#[derive(Debug, Deserialize)]
struct GetBalanceParams {
    address: String, // Hex-encoded 32-byte address
}

/// Result for novai_getBalance.
#[derive(Debug, Serialize)]
struct GetBalanceResult {
    balance: String, // u128 as decimal string (avoids JSON number precision loss)
    nonce: u64,
}

/// Parameters for novai_getAiEntity.
#[derive(Debug, Deserialize)]
struct GetAiEntityParams {
    entity_id: String, // Hex-encoded 32-byte entity ID
}

/// JSON-serializable AI entity.
#[derive(Debug, Serialize)]
struct AiEntityJson {
    id: String,
    code_hash: String,
    creator: String,
    autonomy_mode: u8,
    capabilities: u8,
    economic_balance: String,
    nonce: u64,
    pubkey: String,
    memory_root: String,
    params_root: String,
    registered_at: u64,
    last_active_at: u64,
    is_active: bool,
    reputation_score: u16,
    total_transactions: u32,
    reputation_events_count: u32,
    stake_balance: String,
    stake_locked_until: u64,
    upgrade_count: u32,
    last_upgrade_height: u64,
}

/// Result for novai_getAiEntity.
#[derive(Debug, Serialize)]
struct GetAiEntityResult {
    entity: Option<AiEntityJson>,
}

/// Parameters for novai_getUpgradeHistory.
#[derive(Debug, Deserialize)]
struct GetUpgradeHistoryParams {
    entity_id: String, // Hex-encoded 32-byte entity ID
    start_height: u64,
    end_height: u64,
}

/// JSON-serializable upgrade history row.
#[derive(Debug, Serialize)]
struct UpgradeRecordJson {
    old_code_hash: String,
    new_code_hash: String,
    upgrade_height: u64,
    upgrade_count: u32,
    reason_hash: String,
}

impl UpgradeRecordJson {
    fn from_record(r: UpgradeRecord) -> Self {
        Self {
            old_code_hash: hex::encode(r.old_code_hash),
            new_code_hash: hex::encode(r.new_code_hash),
            upgrade_height: r.upgrade_height,
            upgrade_count: r.upgrade_count,
            reason_hash: hex::encode(r.reason_hash),
        }
    }
}

/// Result for novai_getUpgradeHistory.
#[derive(Debug, Serialize)]
struct GetUpgradeHistoryResult {
    upgrades: Vec<UpgradeRecordJson>,
}

/// Parameters for novai_getOracleAnchorsByEntity (Week 35).
///
/// `start_height`/`end_height` are the inclusive chain-height window
/// (indexed). `ts_min`/`ts_max` are an optional in-memory filter on the
/// external (oracle-attested) timestamp.
#[derive(Debug, Deserialize)]
struct GetOracleAnchorsByEntityParams {
    entity_id: String,
    start_height: u64,
    end_height: u64,
    #[serde(default)]
    ts_min: Option<u64>,
    #[serde(default)]
    ts_max: Option<u64>,
}

/// Parameters for novai_getOracleAnchorsByTag (Week 35). `data_tag` is the
/// raw tag string (1..=32 bytes); it is matched by its domain-separated hash.
#[derive(Debug, Deserialize)]
struct GetOracleAnchorsByTagParams {
    data_tag: String,
    start_height: u64,
    end_height: u64,
    #[serde(default)]
    ts_min: Option<u64>,
    #[serde(default)]
    ts_max: Option<u64>,
}

/// Parameters for novai_getOracleAnchor (point query by signal hash).
#[derive(Debug, Deserialize)]
struct GetOracleAnchorParams {
    signal_hash: String,
}

/// JSON-serializable oracle anchor record. `data_tag` is a lossy UTF-8 view
/// for readability; `data_tag_hex` carries the exact opaque tag bytes.
#[derive(Debug, Serialize)]
struct OracleAnchorJson {
    issuer_entity_id: String,
    data_hash: String,
    external_timestamp: u64,
    source_hash: String,
    expiry_height: u64,
    anchor_height: u64,
    data_tag: String,
    data_tag_hex: String,
}

impl OracleAnchorJson {
    fn from_record(r: OracleAnchorRecord) -> Self {
        Self {
            issuer_entity_id: hex::encode(r.issuer_entity_id),
            data_hash: hex::encode(r.data_hash),
            external_timestamp: r.external_timestamp,
            source_hash: hex::encode(r.source_hash),
            expiry_height: r.expiry_height,
            anchor_height: r.anchor_height,
            data_tag: String::from_utf8_lossy(&r.data_tag).into_owned(),
            data_tag_hex: hex::encode(&r.data_tag),
        }
    }
}

/// Result for the oracle-anchor list methods.
#[derive(Debug, Serialize)]
struct GetOracleAnchorsResult {
    anchors: Vec<OracleAnchorJson>,
}

/// Result for novai_getOracleAnchor (point query).
#[derive(Debug, Serialize)]
struct GetOracleAnchorResult {
    anchor: Option<OracleAnchorJson>,
}

/// True if `ts` falls within the optional inclusive `[ts_min, ts_max]`
/// external-timestamp filter (an absent bound does not constrain).
fn oracle_ts_in_range(ts: u64, ts_min: Option<u64>, ts_max: Option<u64>) -> bool {
    ts_min.is_none_or(|lo| ts >= lo) && ts_max.is_none_or(|hi| ts <= hi)
}

/// Parameters for novai_getMemoryObjects.
#[derive(Debug, Deserialize)]
struct GetMemoryObjectsParams {
    entity_id: String, // Hex-encoded 32-byte entity ID
}

/// JSON-serializable memory object.
#[derive(Debug, Serialize)]
struct MemoryObjectJson {
    object_id: String,
    object_type: u8,
    owner_entity: String,
    created_at: u64,
    updated_at: u64,
    data: String, // Hex-encoded
    data_size: usize,
}

/// Result for novai_getMemoryObjects.
#[derive(Debug, Serialize)]
struct GetMemoryObjectsResult {
    objects: Vec<MemoryObjectJson>,
}

/// Parameters for novai_faucet.
#[derive(Debug, Deserialize)]
struct FaucetParams {
    address: String, // Hex-encoded 32-byte address to fund
}

/// Result for novai_faucet.
#[derive(Debug, Serialize)]
struct FaucetResult {
    txid: String,
    amount: String, // u64 as decimal string
}

/// Faucet account index (dev account 99).
const FAUCET_ACCOUNT_INDEX: usize = 99;

/// Amount dispensed per faucet request.
const FAUCET_AMOUNT: u64 = 10_000_000;

/// Start the JSON-RPC server with state access (Week 14 - D14.5).
///
/// Extended server that supports both transaction submission and state queries.
///
/// # Arguments
/// - `bind_addr` - Address to bind the HTTP server (e.g., "0.0.0.0:9545")
/// - `mempool` - Shared mempool for transaction submission
/// - `nonce_provider` - Provides expected nonces for transaction validation
/// - `db` - Shared state database for queries
/// - `dev_keys` - Whether dev mode is active (enables faucet endpoint)
/// - `faucet_trusted_proxies` - CIDR allowlist for X-Forwarded-For parsing
///   on the public faucet endpoint. Empty (the safe default) means the
///   forwarded-for header is ignored and only the TCP peer IP is trusted.
/// - `faucet_rate_limit_path` - On-disk path for the persistent per-IP
///   faucet cooldown store. The file is created on first dispense and
///   reloaded on every node restart, so the 24h cooldown survives bounces.
///
/// # Errors
/// Returns error if the server cannot bind to the address.
// Refactoring the long parameter list into a config struct is deferred to a
// follow-up: this change set is scoped to adding the trusted-proxy allowlist
// and the persistent rate-limit store, and keeps the public signature
// stable apart from those new final arguments.
#[allow(clippy::too_many_arguments)]
pub fn start_rpc_server_with_state(
    bind_addr: &str,
    mempool: Arc<Mutex<TxMempool>>,
    nonce_provider: Arc<dyn NonceProvider + Send + Sync>,
    db: Arc<Mutex<Storage>>,
    dev_keys: bool,
    blockchain_index: Arc<Mutex<BlockchainIndex>>,
    faucet_key: Option<ed25519_dalek::SigningKey>,
    faucet_trusted_proxies: Vec<CidrBlock>,
    faucet_rate_limit_path: PathBuf,
    peer_manager: Option<Arc<PeerManager>>,
) -> Result<(), String> {
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start RPC server: {e}"))?;

    tracing::info!(%addr, "RPC server listening (with state queries)");

    // C-06: Concurrent request counter to prevent Slowloris / connection exhaustion.
    let active_requests = Arc::new(AtomicUsize::new(0));

    thread::spawn(move || {
        let mut per_ip_limits: HashMap<IpAddr, VecDeque<Instant>> = HashMap::new();
        let mut last_cleanup = Instant::now();
        let nonce = SharedNonceProvider(nonce_provider);
        // H-04: Per-address faucet rate limiting
        let mut faucet_last_dispense: HashMap<[u8; 32], Instant> = HashMap::new();
        let mut faucet_last_global: Option<Instant> = None;
        // PUBLIC FAUCET: per-IP 24h cooldown state, persisted to disk so the
        // cooldown survives node restarts. See faucet_rate_limit module docs
        // for invariants (single-writer, atomic-write, best-effort persist).
        let mut public_faucet_last_dispense =
            FaucetRateLimit::open(faucet_rate_limit_path.clone(), now_unix_secs());

        for mut request in server.incoming_requests() {
            // C-06: Check concurrent request limit before processing.
            let current = active_requests.fetch_add(1, Ordering::SeqCst);
            if current >= MAX_CONCURRENT_RPC {
                active_requests.fetch_sub(1, Ordering::SeqCst);
                tracing::warn!(
                    active = current,
                    limit = MAX_CONCURRENT_RPC,
                    "RPC connection limit reached, returning 503"
                );
                let _ = request.respond(
                    Response::from_string("Service Unavailable — too many concurrent requests")
                        .with_status_code(StatusCode(503)),
                );
                continue;
            }
            // RAII guard: decrement active count when this request scope ends.
            struct RequestGuard<'a>(&'a AtomicUsize);
            impl Drop for RequestGuard<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _req_guard = RequestGuard(&active_requests);

            // Per-IP rate limiting: sliding 1-second window.
            // peer_ip is the TCP-level peer (the proxy if one sits in front).
            // The public faucet path resolves the real client below via
            // resolve_client_ip when faucet_trusted_proxies is configured.
            let peer_ip = request
                .remote_addr()
                .map(std::net::SocketAddr::ip)
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));

            // PUBLIC FAUCET PRE-ROUTER: short-circuit GET /faucet/<address>
            // before the JSON-RPC body parse. The endpoint has its own per-IP
            // 24h cooldown, so it is not subject to the 1Hz RPC rate limit.
            if request.method() == &tiny_http::Method::Get && request.url().starts_with("/faucet/")
            {
                // Resolve the real client through the configured trusted-proxy
                // allowlist. With no trusted proxies (the default) this returns
                // peer_ip unchanged and any X-Forwarded-For value is ignored.
                let faucet_client_ip =
                    resolve_client_ip(peer_ip, request.headers(), &faucet_trusted_proxies);
                let (status, body) = handle_public_faucet(
                    request.url(),
                    faucet_client_ip,
                    &mempool,
                    &nonce,
                    &faucet_key,
                    &mut public_faucet_last_dispense,
                );
                let response = Response::from_string(body)
                    .with_status_code(StatusCode(status))
                    .with_header(
                        "Content-Type: application/json"
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    );
                if let Err(e) = request.respond(response) {
                    tracing::warn!(%e, "Failed to send public faucet response");
                }
                continue;
            }

            if rpc_rate_limited(&mut per_ip_limits, peer_ip, &mut last_cleanup) {
                if let Err(e) = request.respond(
                    Response::from_string("Too Many Requests").with_status_code(StatusCode(429)),
                ) {
                    tracing::warn!(%e, "Failed to send 429 response");
                }
                continue;
            }
            // Read request body (bounded to prevent OOM from oversized requests)
            let mut body = String::new();
            if let Err(e) = request
                .as_reader()
                .take(MAX_RPC_BODY_SIZE as u64 + 1)
                .read_to_string(&mut body)
            {
                if let Err(re) = request.respond(
                    Response::from_string(format!("Failed to read request: {e}"))
                        .with_status_code(StatusCode(400)),
                ) {
                    tracing::warn!(%re, "Failed to send 400 response");
                }
                continue;
            }
            if body.len() > MAX_RPC_BODY_SIZE {
                if let Err(e) = request.respond(
                    Response::from_string("Request body too large")
                        .with_status_code(StatusCode(413)),
                ) {
                    tracing::warn!(%e, "Failed to send 413 response");
                }
                continue;
            }

            // Parse JSON-RPC request
            let rpc_request: RpcRequest = match serde_json::from_str(&body) {
                Ok(req) => req,
                Err(e) => {
                    let error_response = RpcErrorResponse {
                        jsonrpc: "2.0",
                        error: RpcError {
                            code: -32700,
                            message: format!("Parse error: {e}"),
                        },
                        id: serde_json::Value::Null,
                    };
                    if let Err(e) = request.respond(json_response(error_response)) {
                        tracing::warn!(%e, "Failed to send RPC error response");
                    }
                    continue;
                }
            };

            // Verify JSON-RPC 2.0
            if rpc_request.jsonrpc != "2.0" {
                let error_response = RpcErrorResponse {
                    jsonrpc: "2.0",
                    error: RpcError {
                        code: -32600,
                        message: "Invalid Request: jsonrpc must be '2.0'".to_string(),
                    },
                    id: rpc_request.id,
                };
                if let Err(e) = request.respond(json_response(error_response)) {
                    tracing::warn!(%e, "Failed to send RPC error response");
                }
                continue;
            }

            // Route to method handler
            let http_response = match rpc_request.method.as_str() {
                "novai_submitTransaction" => {
                    match handle_submit_tx(&rpc_request, &mempool, &nonce, &peer_manager) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getNonce" => match handle_get_nonce(&rpc_request, &nonce) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                // Week 14 - D14.5: Signal query methods
                "novai_getSignalsByHeight" => {
                    match handle_get_signals_by_height(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getSignalsByIssuer" => {
                    match handle_get_signals_by_issuer(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getSignalsByType" => match handle_get_signals_by_type(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                // Week 28: native x402 payment rail queries.
                "novai_getPaymentsByEntity" => {
                    match handle_get_payments_by_entity(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                // Week 29: Agent Discovery Registry queries.
                "novai_getServiceDescriptorsByCategory" => {
                    match handle_get_service_descriptors_by_category(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                // Week 30: VK Registry queries.
                "novai_getVkRegistration" => match handle_get_vk_registration(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_listVkRegistrations" => {
                    match handle_list_vk_registrations(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getSlaAgreement" => match handle_get_sla_agreement(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_getActiveSla" => match handle_get_active_sla(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_listSlasByBuyer" => match handle_list_slas_by_buyer(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_listSlasBySeller" => match handle_list_slas_by_seller(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_getPaymentChannel" => match handle_get_payment_channel(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_listChannelsByPartyA" => {
                    match handle_list_channels_by_party_a(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_listChannelsByPartyB" => {
                    match handle_list_channels_by_party_b(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getChannelDisputeStatus" => {
                    match handle_get_channel_dispute_status(&rpc_request, &db, &blockchain_index) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getBalance" => match handle_get_balance(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_getAiEntity" => match handle_get_ai_entity(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_getUpgradeHistory" => match handle_get_upgrade_history(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_getOracleAnchorsByEntity" => {
                    match handle_get_oracle_anchors_by_entity(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getOracleAnchorsByTag" => {
                    match handle_get_oracle_anchors_by_tag(&rpc_request, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                "novai_getOracleAnchor" => match handle_get_oracle_anchor(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_getMemoryObjects" => match handle_get_memory_objects(&rpc_request, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result: serde_json::to_value(&result).unwrap(),
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => {
                        let response = RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                },
                "novai_faucet" => {
                    match handle_faucet(
                        &rpc_request,
                        &mempool,
                        &nonce,
                        dev_keys,
                        &faucet_key,
                        &mut faucet_last_dispense,
                        &mut faucet_last_global,
                    ) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result: serde_json::to_value(&result).unwrap(),
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => {
                            let response = RpcErrorResponse {
                                jsonrpc: "2.0",
                                error,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                    }
                }
                // Block explorer endpoints (P1-4 + P1-5)
                "novai_getTransaction" => {
                    match handle_get_transaction(&rpc_request, &blockchain_index, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => json_response(RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        }),
                    }
                }
                "novai_getBlockByHeight" => {
                    match handle_get_block_by_height(&rpc_request, &db, &blockchain_index) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => json_response(RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        }),
                    }
                }
                "novai_getBlockByHash" => {
                    match handle_get_block_by_hash(&rpc_request, &blockchain_index, &db) {
                        Ok(result) => {
                            let response = RpcResponse {
                                jsonrpc: "2.0",
                                result,
                                id: rpc_request.id,
                            };
                            json_response(response)
                        }
                        Err(error) => json_response(RpcErrorResponse {
                            jsonrpc: "2.0",
                            error,
                            id: rpc_request.id,
                        }),
                    }
                }
                "novai_getLatestBlock" => match handle_get_latest_block(&blockchain_index, &db) {
                    Ok(result) => {
                        let response = RpcResponse {
                            jsonrpc: "2.0",
                            result,
                            id: rpc_request.id,
                        };
                        json_response(response)
                    }
                    Err(error) => json_response(RpcErrorResponse {
                        jsonrpc: "2.0",
                        error,
                        id: rpc_request.id,
                    }),
                },
                _ => {
                    let response = RpcErrorResponse {
                        jsonrpc: "2.0",
                        error: RpcError {
                            code: -32601,
                            message: format!("Method not found: {}", rpc_request.method),
                        },
                        id: rpc_request.id,
                    };
                    json_response(response)
                }
            };

            if let Err(e) = request.respond(http_response) {
                tracing::warn!(%e, "Failed to send RPC response");
            }
        }
    });

    Ok(())
}

/// Handle novai_submitTransaction RPC method.
fn handle_submit_tx(
    request: &RpcRequest,
    mempool: &Arc<Mutex<TxMempool>>,
    nonce_provider: &SharedNonceProvider,
    peer_manager: &Option<Arc<PeerManager>>,
) -> Result<SubmitTxResult, RpcError> {
    // Parse parameters
    let params: SubmitTxParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    // Reject oversized hex strings before allocating for decode.
    // MAX_TX_SIZE is 128KB; hex-encoded is 2x = 256KB max hex chars.
    if params.tx.len() > novai_types::MAX_TX_SIZE * 2 {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Transaction hex too large: {} bytes exceeds limit of {} bytes",
                params.tx.len() / 2,
                novai_types::MAX_TX_SIZE,
            ),
        });
    }

    // Decode hex transaction
    let tx_bytes = hex::decode(&params.tx).map_err(|e| RpcError {
        code: -32000,
        message: format!("Invalid hex encoding: {e}"),
    })?;

    // Decode transaction
    let tx: TxV1 = decode_tx_v1_signed(&tx_bytes).map_err(|e| {
        tracing::debug!(?e, "Transaction decoding failed");
        RpcError {
            code: -32000,
            message: "Invalid transaction encoding".to_string(),
        }
    })?;

    // Size limit check (cheapest rejection point — before touching the mempool)
    let tx_size = novai_codec::tx_encoded_size(&tx);
    if tx_size > novai_types::MAX_TX_SIZE {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "transaction too large: {} bytes exceeds limit of {} bytes",
                tx_size,
                novai_types::MAX_TX_SIZE
            ),
        });
    }

    // Compute transaction ID
    let txid = txid_v1(&tx).map_err(|e| {
        tracing::debug!(?e, "Txid computation failed");
        RpcError {
            code: -32000,
            message: "Failed to compute transaction ID".to_string(),
        }
    })?;

    // Submit to mempool
    let mut mempool_guard = mempool.lock_or_recover();
    let size_before = mempool_guard.len();
    match mempool_guard.insert(tx, nonce_provider) {
        Ok(id) => {
            let size_after = mempool_guard.len();
            tracing::debug!(
                txid = %hex::encode(id),
                size_before,
                size_after,
                "RPC tx accepted"
            );
            drop(mempool_guard);

            // Gossip to peers so all validators have txs for proposal
            if let Some(pm) = peer_manager {
                if let Err(e) = pm.broadcast(&NetworkMessage::Transaction(tx_bytes)) {
                    tracing::warn!(?e, "Failed to gossip tx to peers");
                }
            }

            Ok(SubmitTxResult {
                txid: hex::encode(txid),
            })
        }
        Err(e) => {
            tracing::debug!(
                txid = %hex::encode(txid),
                size_before,
                error = %format!("{:?}", e),
                "RPC tx rejected"
            );
            drop(mempool_guard);
            // Gate SOAK C2: count the rejection by reason.
            crate::metrics::pool_metrics::record_rejection(&e);
            // Map mempool errors to distinct codes so clients can distinguish
            // rejection types without leaking internal debug details (H-06).
            // Codes: -32001 = MempoolFull, -32010 = NonceTooLow,
            //        -32011 = FeeTooLow, -32012 = SenderLimitExceeded,
            //        -32013 = other validation error, -32014 = NonceTooHigh
            let (code, message) = match e {
                mempool::TxMempoolError::MempoolFull { .. } => (-32001, "MempoolFull".to_string()),
                mempool::TxMempoolError::NonceTooLow { expected, got } => (
                    -32010,
                    format!("NonceTooLow: expected {expected}, got {got}"),
                ),
                // Gate SOAK A5. Distinct from NonceTooLow because the client
                // response is different: too low means the transaction is
                // dead and the client must resync; too high means it is
                // simply early and the same transaction succeeds once the
                // sender's earlier nonces commit.
                mempool::TxMempoolError::NonceTooHigh {
                    expected,
                    got,
                    horizon,
                } => (
                    -32014,
                    format!("NonceTooHigh: expected {expected}, got {got}, horizon {horizon}"),
                ),
                mempool::TxMempoolError::FeeTooLow { min_fee, got } => {
                    (-32011, format!("FeeTooLow: minimum {min_fee}, got {got}"))
                }
                mempool::TxMempoolError::SenderLimitExceeded { max, .. } => (
                    -32012,
                    format!("SenderLimitExceeded: max {max} pending per sender"),
                ),
                _ => (-32013, "Transaction validation failed".to_string()),
            };
            Err(RpcError { code, message })
        }
    }
}

/// Handle novai_getNonce RPC method.
///
/// Returns the expected next nonce for a given address. The tx-generator
/// uses this to resync after consecutive rejections instead of blindly
/// resetting to 0.
fn handle_get_nonce(
    request: &RpcRequest,
    nonce_provider: &SharedNonceProvider,
) -> Result<GetNonceResult, RpcError> {
    let params: GetNonceParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let addr_bytes = hex::decode(&params.address).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address hex: {e}"),
    })?;
    if addr_bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("Address must be 32 bytes, got {}", addr_bytes.len()),
        });
    }
    let mut address = [0u8; 32];
    address.copy_from_slice(&addr_bytes);

    let nonce = nonce_provider.expected_nonce(&address);
    Ok(GetNonceResult { nonce })
}

// ============================================================================
// SIGNAL QUERY HANDLERS (Week 14 - D14.5)
// ============================================================================

/// Handle novai_getSignalsByHeight RPC method.
fn handle_get_signals_by_height(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<SignalQueryResult, RpcError> {
    let params: GetSignalsByHeightParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let db = db.lock_or_recover();
    let signals = get_signals_by_height(&*db, params.height).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(SignalQueryResult {
        signals: signals
            .into_iter()
            .map(SignalCommitmentJson::from)
            .collect(),
    })
}

/// Handle novai_getSignalsByIssuer RPC method.
fn handle_get_signals_by_issuer(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<SignalQueryResult, RpcError> {
    let params: GetSignalsByIssuerParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    // Decode issuer hex
    let issuer_bytes = hex::decode(&params.issuer).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid issuer hex: {e}"),
    })?;
    if issuer_bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("Issuer must be 32 bytes, got {}", issuer_bytes.len()),
        });
    }
    let mut issuer = [0u8; 32];
    issuer.copy_from_slice(&issuer_bytes);

    let db = db.lock_or_recover();
    let signals = get_signals_by_issuer(&*db, &issuer, params.start_height, params.end_height)
        .map_err(|_| RpcError {
            code: -32002,
            message: "State query failed".to_string(),
        })?;

    Ok(SignalQueryResult {
        signals: signals
            .into_iter()
            .map(SignalCommitmentJson::from)
            .collect(),
    })
}

/// Handle novai_getSignalsByType RPC method.
fn handle_get_signals_by_type(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<SignalQueryResult, RpcError> {
    let params: GetSignalsByTypeParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    // Validate signal type
    let signal_type = AiSignalType::from_byte(params.signal_type).ok_or_else(|| RpcError {
        code: -32602,
        message: format!("Invalid signal type: {} (must be 0-6)", params.signal_type),
    })?;

    let db = db.lock_or_recover();
    let signals = get_signals_by_type(&*db, signal_type, params.start_height, params.end_height)
        .map_err(|_| RpcError {
            code: -32002,
            message: "State query failed".to_string(),
        })?;

    Ok(SignalQueryResult {
        signals: signals
            .into_iter()
            .map(SignalCommitmentJson::from)
            .collect(),
    })
}

/// Handle novai_getPaymentsByEntity RPC method (Week 28).
///
/// Returns every PaymentRecord where `entity_id` is either the payer
/// or the payee (selected by `role`) and `payment_height` falls within
/// `[start_height, end_height]`. The handler enforces the same range
/// cap as the signal queries (`MAX_SIGNAL_QUERY_RANGE` heights).
fn handle_get_payments_by_entity(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetPaymentsByEntityResult, RpcError> {
    let params: GetPaymentsByEntityParams = serde_json::from_value(request.params.clone())
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let role = match params.role.as_str() {
        "payer" => PaymentRole::Payer,
        "payee" => PaymentRole::Payee,
        other => {
            return Err(RpcError {
                code: -32602,
                message: format!("role must be \"payer\" or \"payee\", got {other:?}"),
            });
        }
    };

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;

    let db = db.lock_or_recover();
    let payments = get_payments_with_splits_and_condition_by_entity(
        &*db,
        &entity_id,
        role,
        params.start_height,
        params.end_height,
    )
    .map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetPaymentsByEntityResult {
        payments: payments
            .into_iter()
            .map(|(r, s, c)| PaymentJson::from_record_with_splits_and_condition(r, s, c))
            .collect(),
    })
}

/// Handle novai_getServiceDescriptorsByCategory RPC method (Week 29).
///
/// Returns every published `ServiceDescriptor` whose `category` byte
/// matches the request. Stale index entries are skipped by the
/// underlying helper. No height windowing: the by_category index does
/// not carry a height key, and descriptor counts are bounded by the
/// per-entity cap and the number of publishing entities so a single
/// scan fits comfortably under the RPC response size limit.
fn handle_get_service_descriptors_by_category(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetServiceDescriptorsByCategoryResult, RpcError> {
    let params: GetServiceDescriptorsByCategoryParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let db = db.lock_or_recover();
    let descriptors =
        get_service_descriptors_by_category(&*db, params.category).map_err(|_| RpcError {
            code: -32002,
            message: "State query failed".to_string(),
        })?;

    Ok(GetServiceDescriptorsByCategoryResult {
        descriptors: descriptors
            .into_iter()
            .map(ServiceDescriptorJson::from)
            .collect(),
    })
}

/// Handle novai_getVkRegistration RPC method (Week 30).
///
/// Resolves a single `VkRegistration` by its 32-byte memory object id.
/// Returns `{ registration: null }` if the handle does not currently
/// resolve (never existed, deleted, or stale state). The full
/// compressed VK is included in the response so clients can use it
/// directly for off-chain verification or replay.
fn handle_get_vk_registration(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetVkRegistrationResult, RpcError> {
    let params: GetVkRegistrationParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let id = parse_hex32(&params.id, "id")?;

    let db = db.lock_or_recover();
    let resolved = get_vk_registration_by_id(&*db, &id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetVkRegistrationResult {
        registration: resolved.map(VkRegistrationJson::from),
    })
}

/// Handle novai_listVkRegistrations RPC method (Week 30).
///
/// Returns every `VkRegistration` owned by `entity_id`, in
/// big-endian-object-id-ascending order (the natural lex order of the
/// memory-by-type index). The result list is bounded by
/// `MAX_VK_REGISTRATIONS_PER_ENTITY` (= 8 in v1), so the response size
/// is naturally small.
fn handle_list_vk_registrations(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<ListVkRegistrationsResult, RpcError> {
    let params: ListVkRegistrationsParams = serde_json::from_value(request.params.clone())
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;

    let db = db.lock_or_recover();
    let registrations = get_vk_registrations_by_entity(&*db, &entity_id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(ListVkRegistrationsResult {
        registrations: registrations
            .into_iter()
            .map(VkRegistrationJson::from)
            .collect(),
    })
}

/// Handle novai_getSlaAgreement RPC method (Week 31).
///
/// Resolves a single SLA by its `(owner, object_id)` pair. Returns
/// `{ agreement: null }` if the memory object does not exist, the
/// type does not match, or the payload fails to decode.
fn handle_get_sla_agreement(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetSlaAgreementResult, RpcError> {
    let params: GetSlaAgreementParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let owner = parse_hex32(&params.owner, "owner")?;
    let object_id = parse_hex32(&params.object_id, "object_id")?;

    let db = db.lock_or_recover();
    let resolved = get_sla_agreement(&*db, &owner, &object_id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetSlaAgreementResult {
        agreement: resolved.map(SlaAgreementJson::from),
    })
}

/// Handle novai_getActiveSla RPC method (Week 31).
///
/// Resolves the currently-open SLA between `(buyer, seller)` via the
/// active-between singleton. Returns `{ agreement: null }` if no
/// open SLA exists for the pair.
fn handle_get_active_sla(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetSlaAgreementResult, RpcError> {
    let params: GetActiveSlaParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let buyer = parse_hex32(&params.buyer, "buyer")?;
    let seller = parse_hex32(&params.seller, "seller")?;

    let db = db.lock_or_recover();
    let resolved = get_active_sla_between(&*db, &buyer, &seller).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetSlaAgreementResult {
        agreement: resolved.map(SlaAgreementJson::from),
    })
}

/// Handle novai_listSlasByBuyer RPC method (Week 31).
///
/// Returns every SLA where the entity is the buyer and the SLA was
/// created in `[start_height, end_height]` (inclusive). The height
/// window is capped at `MAX_SIGNAL_QUERY_RANGE` matching the other
/// height-windowed queries.
fn handle_list_slas_by_buyer(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<ListSlasResult, RpcError> {
    let params: ListSlasParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let db = db.lock_or_recover();
    let pairs = get_slas_by_buyer(&*db, &entity_id, params.start_height, params.end_height)
        .map_err(|_| RpcError {
            code: -32002,
            message: "State query failed".to_string(),
        })?;

    Ok(ListSlasResult {
        agreements: pairs.into_iter().map(SlaAgreementJson::from).collect(),
    })
}

/// Handle novai_listSlasBySeller RPC method (Week 31).
///
/// Returns every SLA where the entity is the seller and the SLA was
/// created in `[start_height, end_height]`. Unlike the by-buyer
/// query the per-entry resolution walks the by-type index to recover
/// the buyer (memory-object owner); the lookup is bounded by the
/// total number of `SlaAgreement` memory objects per buyer (cap = 8).
fn handle_list_slas_by_seller(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<ListSlasResult, RpcError> {
    let params: ListSlasParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let db = db.lock_or_recover();
    let pairs = get_slas_by_seller(&*db, &entity_id, params.start_height, params.end_height)
        .map_err(|_| RpcError {
            code: -32002,
            message: "State query failed".to_string(),
        })?;

    Ok(ListSlasResult {
        agreements: pairs.into_iter().map(SlaAgreementJson::from).collect(),
    })
}

/// Handle novai_getPaymentChannel RPC method (Week 32).
///
/// Resolves a single `PaymentChannel` by its `(owner, object_id)`
/// pair. Returns `{ channel: null }` if the memory object does not
/// exist, the type does not match, or the payload fails to decode.
fn handle_get_payment_channel(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetPaymentChannelResult, RpcError> {
    let params: GetPaymentChannelParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let owner = parse_hex32(&params.owner, "owner")?;
    let object_id = parse_hex32(&params.object_id, "object_id")?;

    let db = db.lock_or_recover();
    let resolved = get_payment_channel(&*db, &owner, &object_id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetPaymentChannelResult {
        channel: resolved.map(PaymentChannelJson::from),
    })
}

/// Handle novai_listChannelsByPartyA RPC method (Week 32).
///
/// Returns every `PaymentChannel` whose memory-object owner is the
/// queried entity and whose `created_at` falls inside
/// `[start_height, end_height]` (inclusive). The height window is
/// capped at `MAX_SIGNAL_QUERY_RANGE` matching the other
/// height-windowed queries.
fn handle_list_channels_by_party_a(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<ListChannelsResult, RpcError> {
    let params: ListChannelsParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let db = db.lock_or_recover();
    let pairs = get_channels_by_party_a(&*db, &entity_id, params.start_height, params.end_height)
        .map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(ListChannelsResult {
        channels: pairs.into_iter().map(PaymentChannelJson::from).collect(),
    })
}

/// Handle novai_listChannelsByPartyB RPC method (Week 32).
///
/// Returns every `PaymentChannel` where the queried entity is the
/// embedded counterparty (party B) and the `created_at` falls inside
/// `[start_height, end_height]`. The by-party-B index value embeds
/// the channel's memory-object owner (party A), so primary-record
/// resolution is O(1) per match (no walk through the by-type index).
fn handle_list_channels_by_party_b(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<ListChannelsResult, RpcError> {
    let params: ListChannelsParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let db = db.lock_or_recover();
    let pairs = get_channels_by_party_b(&*db, &entity_id, params.start_height, params.end_height)
        .map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(ListChannelsResult {
        channels: pairs.into_iter().map(PaymentChannelJson::from).collect(),
    })
}

/// Handle novai_getChannelDisputeStatus RPC method (Week 32).
///
/// Returns dispute-window-relevant fields plus a derived
/// `blocks_remaining` and `finalize_ready` so monitoring tools and
/// the CLI do not have to combine a separate `novai_getStatus` call
/// with the channel record. Reads `committed_height` from the
/// blockchain index for the comparison.
fn handle_get_channel_dispute_status(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
    index: &Arc<Mutex<BlockchainIndex>>,
) -> Result<GetChannelDisputeStatusResult, RpcError> {
    let params: GetPaymentChannelParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;
    let owner = parse_hex32(&params.owner, "owner")?;
    let object_id = parse_hex32(&params.object_id, "object_id")?;

    let current_height = index.lock_or_recover().committed_height;
    let db = db.lock_or_recover();
    let resolved = get_payment_channel(&*db, &owner, &object_id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    let Some((_, channel)) = resolved else {
        return Ok(GetChannelDisputeStatusResult {
            found: false,
            status: 0,
            status_label: "unknown",
            closing_at_height: 0,
            dispute_deadline_height: 0,
            current_height,
            blocks_remaining: 0,
            finalize_ready: false,
        });
    };

    // blocks_remaining is meaningful only when the channel is in
    // CLOSING. For other statuses we return 0 (and finalize_ready is
    // necessarily false).
    let (blocks_remaining, finalize_ready) = if channel.status == PAYMENT_CHANNEL_STATUS_CLOSING {
        let remaining = channel
            .dispute_deadline_height
            .saturating_sub(current_height);
        let ready = current_height > channel.dispute_deadline_height;
        (remaining, ready)
    } else {
        (0, false)
    };

    Ok(GetChannelDisputeStatusResult {
        found: true,
        status: channel.status,
        status_label: payment_channel_status_label(channel.status),
        closing_at_height: channel.closing_at_height,
        dispute_deadline_height: channel.dispute_deadline_height,
        current_height,
        blocks_remaining,
        finalize_ready,
    })
}

// ============================================================================
// STATE QUERY HANDLERS (CLI support)
// ============================================================================

/// Parse and validate a 32-byte hex-encoded value.
///
/// L-02: Hex values are case-insensitive (both "ab" and "AB" are valid).
/// This is standard hex behavior and intentional — API consumers should
/// normalize to lowercase for consistency but uppercase is accepted.
fn parse_hex32(hex_str: &str, field_name: &str) -> Result<[u8; 32], RpcError> {
    let bytes = hex::decode(hex_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid {field_name} hex: {e}"),
    })?;
    if bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("{} must be 32 bytes, got {}", field_name, bytes.len()),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Handle novai_getBalance RPC method.
fn handle_get_balance(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetBalanceResult, RpcError> {
    let params: GetBalanceParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let address = parse_hex32(&params.address, "address")?;
    let db = db.lock_or_recover();
    let account = read_account_or_default(&*db, &address).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetBalanceResult {
        balance: account.balance.to_string(),
        nonce: account.nonce,
    })
}

/// Handle novai_getAiEntity RPC method.
fn handle_get_ai_entity(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetAiEntityResult, RpcError> {
    let params: GetAiEntityParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    let db = db.lock_or_recover();
    let entity = read_ai_entity(&*db, &entity_id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    let entity_json = match entity {
        None => None,
        Some(e) => {
            // Join the per-entity upgrade summary (absent => never upgraded).
            let summary = read_upgrade_summary(&*db, &entity_id).map_err(|_| RpcError {
                code: -32002,
                message: "State query failed".to_string(),
            })?;
            let (upgrade_count, last_upgrade_height) =
                summary.map_or((0, 0), |s| (s.upgrade_count, s.last_upgrade_height));
            Some(AiEntityJson {
                id: hex::encode(e.id),
                code_hash: hex::encode(e.code_hash),
                creator: hex::encode(e.creator),
                autonomy_mode: e.autonomy_mode.to_byte(),
                capabilities: e.capabilities.to_byte(),
                economic_balance: e.economic_balance.to_string(),
                nonce: e.nonce,
                pubkey: hex::encode(e.pubkey),
                memory_root: hex::encode(e.memory_root),
                params_root: hex::encode(e.params_root),
                registered_at: e.registered_at,
                last_active_at: e.last_active_at,
                is_active: e.is_active,
                reputation_score: e.reputation_score,
                total_transactions: e.total_transactions,
                reputation_events_count: e.reputation_events_count,
                stake_balance: e.stake_balance.to_string(),
                stake_locked_until: e.stake_locked_until,
                upgrade_count,
                last_upgrade_height,
            })
        }
    };

    Ok(GetAiEntityResult {
        entity: entity_json,
    })
}

/// Handle novai_getUpgradeHistory RPC method (Week 34).
///
/// Returns the entity's `UpgradeRecord` rows whose `upgrade_height` falls within
/// `[start_height, end_height]`, ascending by height. Enforces the same range
/// cap as the other height-windowed queries (`MAX_SIGNAL_QUERY_RANGE` heights).
fn handle_get_upgrade_history(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetUpgradeHistoryResult, RpcError> {
    let params: GetUpgradeHistoryParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    let db = db.lock_or_recover();
    let records = get_upgrade_history(&*db, &entity_id, params.start_height, params.end_height)
        .map_err(|_| RpcError {
            code: -32002,
            message: "State query failed".to_string(),
        })?;

    Ok(GetUpgradeHistoryResult {
        upgrades: records
            .into_iter()
            .map(UpgradeRecordJson::from_record)
            .collect(),
    })
}

/// Handle novai_getOracleAnchorsByEntity RPC method (Week 35).
fn handle_get_oracle_anchors_by_entity(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetOracleAnchorsResult, RpcError> {
    let params: GetOracleAnchorsByEntityParams = serde_json::from_value(request.params.clone())
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    let db = db.lock_or_recover();
    let records =
        get_oracle_anchors_by_entity(&*db, &entity_id, params.start_height, params.end_height)
            .map_err(|_| RpcError {
                code: -32002,
                message: "State query failed".to_string(),
            })?;

    Ok(GetOracleAnchorsResult {
        anchors: records
            .into_iter()
            .filter(|r| oracle_ts_in_range(r.external_timestamp, params.ts_min, params.ts_max))
            .map(OracleAnchorJson::from_record)
            .collect(),
    })
}

/// Handle novai_getOracleAnchorsByTag RPC method (Week 35).
fn handle_get_oracle_anchors_by_tag(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetOracleAnchorsResult, RpcError> {
    let params: GetOracleAnchorsByTagParams = serde_json::from_value(request.params.clone())
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {MAX_SIGNAL_QUERY_RANGE} heights per query"
            ),
        });
    }

    let tag_bytes = params.data_tag.as_bytes();
    if tag_bytes.is_empty() || tag_bytes.len() > ORACLE_ANCHOR_DATA_TAG_MAX_LEN {
        return Err(RpcError {
            code: -32602,
            message: format!("data_tag must be 1..={ORACLE_ANCHOR_DATA_TAG_MAX_LEN} bytes"),
        });
    }

    let db = db.lock_or_recover();
    let records =
        get_oracle_anchors_by_tag(&*db, tag_bytes, params.start_height, params.end_height)
            .map_err(|_| RpcError {
                code: -32002,
                message: "State query failed".to_string(),
            })?;

    Ok(GetOracleAnchorsResult {
        anchors: records
            .into_iter()
            .filter(|r| oracle_ts_in_range(r.external_timestamp, params.ts_min, params.ts_max))
            .map(OracleAnchorJson::from_record)
            .collect(),
    })
}

/// Handle novai_getOracleAnchor RPC method (Week 35, point query).
fn handle_get_oracle_anchor(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetOracleAnchorResult, RpcError> {
    let params: GetOracleAnchorParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let signal_hash = parse_hex32(&params.signal_hash, "signal_hash")?;
    let db = db.lock_or_recover();
    let record = get_oracle_anchor(&*db, &signal_hash).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetOracleAnchorResult {
        anchor: record.map(OracleAnchorJson::from_record),
    })
}

/// Handle novai_getMemoryObjects RPC method.
fn handle_get_memory_objects(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
) -> Result<GetMemoryObjectsResult, RpcError> {
    let params: GetMemoryObjectsParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    let db = db.lock_or_recover();
    let objects = get_memory_objects_by_entity(&*db, &entity_id).map_err(|_| RpcError {
        code: -32002,
        message: "State query failed".to_string(),
    })?;

    Ok(GetMemoryObjectsResult {
        objects: objects
            .into_iter()
            .map(|o| MemoryObjectJson {
                object_id: hex::encode(o.object_id),
                object_type: o.object_type.to_byte(),
                owner_entity: hex::encode(o.owner_entity),
                created_at: o.created_at,
                updated_at: o.updated_at,
                data_size: o.data.len(),
                data: hex::encode(o.data),
            })
            .collect(),
    })
}

/// Handle novai_faucet RPC method.
///
/// Creates and submits a transfer from the dev faucet account (index 99).
/// Only active when `dev_keys` is true.
/// Minimum seconds between faucet calls for the same address.
const FAUCET_PER_ADDRESS_COOLDOWN_SECS: u64 = 3600; // 1 hour
/// Minimum seconds between any faucet call (global cooldown).
const FAUCET_GLOBAL_COOLDOWN_SECS: u64 = 10;

fn handle_faucet(
    request: &RpcRequest,
    mempool: &Arc<Mutex<TxMempool>>,
    nonce_provider: &SharedNonceProvider,
    dev_keys: bool,
    faucet_key: &Option<ed25519_dalek::SigningKey>,
    faucet_last_dispense: &mut HashMap<[u8; 32], Instant>,
    faucet_last_global: &mut Option<Instant>,
) -> Result<FaucetResult, RpcError> {
    // C-04: Faucet key resolution:
    // 1. If --faucet-key was provided: use the loaded key
    // 2. If dev-mode: fall back to deterministic dev key (local dev only)
    // 3. Otherwise: faucet is disabled
    let faucet_sk = if let Some(ref key) = faucet_key {
        key.clone()
    } else if dev_keys {
        // Deterministic dev key — acceptable for local development only.
        // This key is trivially recoverable from source code.
        let seed_byte = (FAUCET_ACCOUNT_INDEX % 256) as u8;
        let mut seed = [seed_byte; 32];
        let index_bytes = FAUCET_ACCOUNT_INDEX.to_le_bytes();
        for (j, &b) in index_bytes.iter().enumerate() {
            seed[j] ^= b;
        }
        ed25519_dalek::SigningKey::from_bytes(&seed)
    } else {
        return Err(RpcError {
            code: -32000,
            message: "Faucet disabled. Use --faucet-key <path> or --dev-keys to enable."
                .to_string(),
        });
    };

    let params: FaucetParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let to_address = parse_hex32(&params.address, "address")?;

    // H-04: Global cooldown — prevent rapid-fire faucet calls
    let now = Instant::now();
    if let Some(last) = faucet_last_global {
        let elapsed = now.duration_since(*last);
        if elapsed < Duration::from_secs(FAUCET_GLOBAL_COOLDOWN_SECS) {
            let remaining = FAUCET_GLOBAL_COOLDOWN_SECS - elapsed.as_secs();
            return Err(RpcError {
                code: -32000,
                message: format!("Faucet global cooldown: try again in {remaining} seconds"),
            });
        }
    }

    // H-04: Per-address cooldown — max 1 dispense per address per hour
    if let Some(&last) = faucet_last_dispense.get(&to_address) {
        let elapsed = now.duration_since(last);
        if elapsed < Duration::from_secs(FAUCET_PER_ADDRESS_COOLDOWN_SECS) {
            let remaining = FAUCET_PER_ADDRESS_COOLDOWN_SECS - elapsed.as_secs();
            return Err(RpcError {
                code: -32000,
                message: format!(
                    "Faucet rate limit: address already received tokens. Try again in {remaining} seconds"
                ),
            });
        }
    }

    let txid = dispense_transfer(
        &faucet_sk,
        &to_address,
        FAUCET_AMOUNT,
        mempool,
        nonce_provider,
    )?;

    tracing::info!(
        to = %hex::encode(to_address),
        amount = FAUCET_AMOUNT,
        txid = %hex::encode(txid),
        "Faucet dispensed tokens"
    );

    // H-04: Record successful dispense for rate limiting
    faucet_last_dispense.insert(to_address, now);
    *faucet_last_global = Some(now);

    Ok(FaucetResult {
        txid: hex::encode(txid),
        amount: FAUCET_AMOUNT.to_string(),
    })
}

/// Build, sign, and submit a Type 1 transfer from the faucet account to `to_address`.
///
/// Shared by the dev-mode `novai_faucet` JSON-RPC method and the public
/// `GET /faucet/<address>` HTTP endpoint so both flows take the same
/// build-sign-mempool path. The fee is fixed at MIN_FEE_TRANSFER (100).
fn dispense_transfer(
    faucet_sk: &ed25519_dalek::SigningKey,
    to_address: &[u8; 32],
    amount: u64,
    mempool: &Arc<Mutex<TxMempool>>,
    nonce_provider: &SharedNonceProvider,
) -> Result<[u8; 32], RpcError> {
    let faucet_pk = faucet_sk.verifying_key();
    let faucet_addr = address_from_pubkey(&faucet_pk);

    let nonce = nonce_provider.expected_nonce(&faucet_addr);

    // Transfer payload: [version:1][to:32][amount:8 BE]
    let mut payload = Vec::with_capacity(41);
    payload.push(1);
    payload.extend_from_slice(to_address);
    payload.extend_from_slice(&amount.to_be_bytes());

    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: faucet_addr,
        pubkey: faucet_pk.to_bytes(),
        nonce,
        fee: 100, // MIN_FEE_TRANSFER
        payload,
        sig: [0u8; 64],
    };

    sign_tx_v1(faucet_sk, &mut tx).map_err(|_| RpcError {
        code: -32000,
        message: "Faucet transaction signing failed".to_string(),
    })?;

    let txid = txid_v1(&tx).map_err(|e| {
        tracing::debug!(?e, "Txid computation failed");
        RpcError {
            code: -32000,
            message: "Failed to compute transaction ID".to_string(),
        }
    })?;

    let mut mempool_guard = mempool.lock_or_recover();
    mempool_guard
        .insert(tx, nonce_provider)
        .map_err(|_| RpcError {
            code: -32001,
            message: "Faucet transaction rejected by mempool".to_string(),
        })?;
    drop(mempool_guard);

    Ok(txid)
}

// ============================================================================
// PUBLIC HTTP FAUCET (GET /faucet/<address>)
// ============================================================================
//
// Separate from the dev-mode JSON-RPC novai_faucet method. The public endpoint
// is reachable over plain HTTP and rate-limited per client IP. The faucet key
// loader is unchanged: pass --faucet-key <path> to enable it. Without a
// faucet key the endpoint returns 503.
//
// Rate-limit state (per-IP HashMap) lives on the server-thread stack frame
// and RESETS ON NODE RESTART. Acceptable for v0 of the public devnet faucet;
// a persistent store can replace it later.
//
// X-FORWARDED-FOR HANDLING:
//
// When deployed behind a reverse proxy (nginx, Cloudflare), request.remote_addr()
// returns the proxy IP, not the real client. Parsing X-Forwarded-For without
// restriction is unsafe (any client can spoof the header), so the operator must
// explicitly enumerate trusted proxies via repeatable --faucet-trusted-proxy <CIDR>
// flags. With no trusted proxies (the default) the forwarded-for header is ignored
// and only the TCP peer IP is trusted. See resolve_client_ip for the rightward-walk
// algorithm.

/// A single CIDR block (IPv4 or IPv6) on the faucet trusted-proxy allowlist.
///
/// Used by resolve_client_ip to decide whether the TCP peer is allowed to set
/// X-Forwarded-For on a faucet request, and to decide whether each intermediate
/// hop in the XFF chain is also trusted.
///
/// CIDR matching is implemented inline (~30 LOC of bit math on u32 / u128) so
/// that the node binary does not take a new direct dependency just for this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidrBlock {
    network: IpAddr,
    prefix_len: u8,
}

impl CidrBlock {
    /// Parse a CIDR block such as `10.0.0.0/8` or `2001:db8::/32`.
    ///
    /// Returns `Err(message)` if the form is malformed, the address does not
    /// parse, the prefix length is missing or non-numeric, or the prefix
    /// length exceeds the address family's maximum (32 for v4, 128 for v6).
    pub fn parse(s: &str) -> Result<Self, String> {
        let (ip_str, prefix_str) = s
            .split_once('/')
            .ok_or_else(|| format!("CIDR missing '/<prefix>': {s}"))?;
        let network: IpAddr = ip_str
            .parse()
            .map_err(|e| format!("invalid CIDR address '{ip_str}': {e}"))?;
        let prefix_len: u8 = prefix_str
            .parse()
            .map_err(|e| format!("invalid CIDR prefix '{prefix_str}': {e}"))?;
        let max = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix_len > max {
            return Err(format!(
                "CIDR prefix /{prefix_len} exceeds max /{max} for address family"
            ));
        }
        Ok(CidrBlock {
            network,
            prefix_len,
        })
    }

    /// Test whether an IP falls inside this CIDR block. v4 only matches v4
    /// and v6 only matches v6 (no IPv4-mapped-IPv6 confusion).
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                if self.prefix_len == 0 {
                    return true;
                }
                let mask: u32 = !0u32 << (32 - self.prefix_len);
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                if self.prefix_len == 0 {
                    return true;
                }
                let mask: u128 = !0u128 << (128 - self.prefix_len);
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

/// Resolve the real client IP given the TCP peer IP, the request headers,
/// and the configured trusted-proxy allowlist.
///
/// SEMANTICS:
/// - If `trusted_proxies` does not cover `peer_ip`, X-Forwarded-For is ignored
///   entirely and `peer_ip` is returned. This is the safe default because any
///   external client can forge the header otherwise.
/// - If `peer_ip` is trusted, the X-Forwarded-For chain is walked
///   rightmost-to-leftmost. Each entry that is itself in `trusted_proxies` is
///   treated as another trusted hop; the first entry that is NOT trusted is
///   the real client and is returned.
/// - If every entry in the chain is trusted (no untrusted entry is ever
///   reached), the leftmost entry is returned. This is the original sender
///   from inside the trust boundary and is the standard X-Forwarded-For
///   interpretation.
/// - Malformed entries, empty values, and a missing header all fall back to
///   `peer_ip`. The function never panics on hostile input.
pub(crate) fn resolve_client_ip(
    peer_ip: IpAddr,
    headers: &[tiny_http::Header],
    trusted_proxies: &[CidrBlock],
) -> IpAddr {
    if !ip_is_trusted(peer_ip, trusted_proxies) {
        return peer_ip;
    }
    let Some(xff) = find_xff_header(headers) else {
        return peer_ip;
    };
    let parsed: Vec<IpAddr> = xff
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .collect();
    if parsed.is_empty() {
        return peer_ip;
    }
    for ip in parsed.iter().rev() {
        if !ip_is_trusted(*ip, trusted_proxies) {
            return *ip;
        }
    }
    parsed[0]
}

fn ip_is_trusted(ip: IpAddr, trusted_proxies: &[CidrBlock]) -> bool {
    trusted_proxies.iter().any(|cidr| cidr.contains(ip))
}

fn find_xff_header(headers: &[tiny_http::Header]) -> Option<&str> {
    headers
        .iter()
        .find(|h| h.field.equiv("X-Forwarded-For"))
        .map(|h| h.value.as_str())
}

/// Drip amount per public faucet request (100K NOVAI). Sized for a developer
/// to register an entity (5K fee), post a handful of signals (1K each), make
/// a few payments, and try an entity upgrade without running dry.
const PUBLIC_FAUCET_AMOUNT: u64 = 100_000;

/// Per-IP cooldown for the public faucet (24 hours).
const PUBLIC_FAUCET_PER_IP_COOLDOWN_SECS: u64 = 24 * 3600;

/// Success body for GET /faucet/<address>.
#[derive(Debug, Serialize)]
struct PublicFaucetSuccess {
    txid: String,
    amount: String,
    to: String,
}

/// Error body for GET /faucet/<address>. `retry_after_secs` is populated only
/// on 429 responses.
#[derive(Debug, Serialize)]
struct PublicFaucetError {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
}

/// Extract the 32-byte address from a `/faucet/<address>` URL path.
///
/// Accepts an optional trailing slash. Returns `(http_status, message)` on
/// failure so the caller can emit a matching response.
fn parse_public_faucet_path(url: &str) -> Result<[u8; 32], (u16, String)> {
    const PREFIX: &str = "/faucet/";
    let tail = url
        .strip_prefix(PREFIX)
        .ok_or((404, "Not Found".to_string()))?;
    let addr_hex = tail.trim_end_matches('/');
    if addr_hex.len() != 64 {
        return Err((
            400,
            format!("address must be 64 hex chars, got {}", addr_hex.len()),
        ));
    }
    let bytes = hex::decode(addr_hex).map_err(|e| (400, format!("invalid address hex: {e}")))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Returns `Some(remaining_secs)` if `client_ip` is still within the per-IP
/// cooldown window, `None` if a new dispense is allowed.
///
/// Timestamps are UNIX seconds (u64) rather than `Instant` so the per-IP
/// map can be persisted to disk across node restarts via the
/// `faucet_rate_limit` module; `Instant` is monotonic and not serializable.
fn public_faucet_cooldown_remaining(last_dispense: Option<u64>, now_secs: u64) -> Option<u64> {
    let last = last_dispense?;
    // saturating_sub: if a stored timestamp is somehow in the future (e.g.
    // the wall clock jumped backwards between persist and reload), treat
    // elapsed as zero so the IP is still inside the cooldown window.
    let elapsed = now_secs.saturating_sub(last);
    if elapsed < PUBLIC_FAUCET_PER_IP_COOLDOWN_SECS {
        Some(PUBLIC_FAUCET_PER_IP_COOLDOWN_SECS - elapsed)
    } else {
        None
    }
}

fn public_faucet_error_body(error: impl Into<String>, retry_after_secs: Option<u64>) -> String {
    serde_json::to_string(&PublicFaucetError {
        error: error.into(),
        retry_after_secs,
    })
    .unwrap_or_else(|_| r#"{"error":"serialization failure"}"#.to_string())
}

/// Current wall-clock time in UNIX seconds. Used for faucet rate-limit
/// timestamps which must survive node restarts (so `Instant` cannot apply).
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        // If the system clock is somehow before the UNIX epoch the cooldown
        // simply behaves as if the IP has never dispensed before.
        .unwrap_or(0)
}

/// Handle a `GET /faucet/<address>` request.
///
/// Returns `(http_status, json_body)`. Address parsing runs first so malformed
/// inputs cannot consume an IP's daily slot. Rate-limit accounting is recorded
/// only after the mempool accepts the transaction.
fn handle_public_faucet(
    url: &str,
    client_ip: IpAddr,
    mempool: &Arc<Mutex<TxMempool>>,
    nonce_provider: &SharedNonceProvider,
    faucet_key: &Option<ed25519_dalek::SigningKey>,
    per_ip_last_dispense: &mut FaucetRateLimit,
) -> (u16, String) {
    // 1. Parse + validate the address BEFORE any rate-limit accounting or signing.
    let to_address = match parse_public_faucet_path(url) {
        Ok(addr) => addr,
        Err((status, msg)) => return (status, public_faucet_error_body(msg, None)),
    };

    // 2. Reject early if no faucet key has been loaded.
    let Some(faucet_sk) = faucet_key.as_ref() else {
        return (
            503,
            public_faucet_error_body(
                "Faucet disabled. Operator must start the node with --faucet-key <path>.",
                None,
            ),
        );
    };

    // 3. Per-IP 24h cooldown.
    let now_secs = now_unix_secs();
    if let Some(remaining) =
        public_faucet_cooldown_remaining(per_ip_last_dispense.last_dispense(client_ip), now_secs)
    {
        return (
            429,
            public_faucet_error_body(
                format!(
                    "Rate limit: one request per IP per 24h. Try again in {remaining} seconds."
                ),
                Some(remaining),
            ),
        );
    }

    // 4. Dispense via the shared transfer-build path.
    let txid = match dispense_transfer(
        faucet_sk,
        &to_address,
        PUBLIC_FAUCET_AMOUNT,
        mempool,
        nonce_provider,
    ) {
        Ok(t) => t,
        Err(rpc_err) => {
            // -32001 = mempool rejection (treat as transient: 503). All other
            // codes from dispense_transfer are signing or txid failures (500).
            let status = if rpc_err.code == -32001 { 503 } else { 500 };
            return (status, public_faucet_error_body(rpc_err.message, None));
        }
    };

    // 5. Record dispense ONLY after success, so failed attempts do not burn
    //    an IP's daily slot. The record also persists to disk so the cooldown
    //    survives node restarts (FaucetRateLimit handles atomic writes and
    //    logs persist failures without crashing).
    per_ip_last_dispense.record(client_ip, now_secs);

    tracing::info!(
        ip = %client_ip,
        to = %hex::encode(to_address),
        amount = PUBLIC_FAUCET_AMOUNT,
        txid = %hex::encode(txid),
        "Public faucet dispensed tokens"
    );

    let body = PublicFaucetSuccess {
        txid: hex::encode(txid),
        amount: PUBLIC_FAUCET_AMOUNT.to_string(),
        to: hex::encode(to_address),
    };
    (
        200,
        serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string()),
    )
}

// ============================================================================
// BLOCK EXPLORER HANDLERS (P1-4 + P1-5)
// ============================================================================

/// Handle novai_getTransaction — lookup tx receipt by txid.
fn handle_get_transaction(
    request: &RpcRequest,
    index: &Arc<Mutex<BlockchainIndex>>,
    db: &Arc<Mutex<Storage>>,
) -> Result<serde_json::Value, RpcError> {
    let params: GetTxParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let txid = parse_hex32(&params.txid, "txid")?;
    let idx = index.lock_or_recover();
    let Some(&(height, tx_index)) = idx.tx_receipts.get(&txid) else {
        return Ok(serde_json::Value::Null);
    };
    drop(idx);

    // Load block to get tx details
    let db_guard = db.lock_or_recover();
    let block = novai_consensus::ConsensusState::load_block(&*db_guard, height)
        .map_err(|_| RpcError {
            code: -32002,
            message: "Failed to load block".to_string(),
        })?
        .ok_or_else(|| RpcError {
            code: -32002,
            message: format!("Block at height {height} not found (pruned?)"),
        })?;
    drop(db_guard);

    let tx = block.txs.get(tx_index).ok_or_else(|| RpcError {
        code: -32002,
        message: format!("Tx index {tx_index} out of range in block {height}"),
    })?;

    Ok(serde_json::to_value(GetTxResult {
        block_height: height,
        tx_index,
        from: hex::encode(tx.from),
        nonce: tx.nonce,
        fee: tx.fee,
        payload_len: tx.payload.len(),
    })
    .unwrap())
}

/// Handle novai_getBlockByHeight — return block header by height.
fn handle_get_block_by_height(
    request: &RpcRequest,
    db: &Arc<Mutex<Storage>>,
    index: &Arc<Mutex<BlockchainIndex>>,
) -> Result<serde_json::Value, RpcError> {
    let params: GetBlockByHeightParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    // M-03: Reject queries beyond committed height to avoid expensive failed lookups
    let committed = index.lock_or_recover().committed_height;
    if params.height > committed {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height {} exceeds committed height {}",
                params.height, committed
            ),
        });
    }

    let db_guard = db.lock_or_recover();
    let block =
        novai_consensus::ConsensusState::load_block(&*db_guard, params.height).map_err(|_| {
            RpcError {
                code: -32002,
                message: "Failed to load block".to_string(),
            }
        })?;
    drop(db_guard);

    match block {
        Some(b) => {
            let hash = novai_consensus_types::codec::hash_block_v1(&b).map_err(|_| RpcError {
                code: -32002,
                message: "Failed to hash block".to_string(),
            })?;
            Ok(serde_json::to_value(BlockResult {
                height: b.height,
                round: b.round,
                block_hash: hex::encode(hash),
                parent_hash: hex::encode(b.parent_hash),
                state_root: hex::encode(b.state_root),
                tx_count: b.txs.len(),
            })
            .unwrap())
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// Handle novai_getBlockByHash — lookup block height from hash index, then load.
fn handle_get_block_by_hash(
    request: &RpcRequest,
    index: &Arc<Mutex<BlockchainIndex>>,
    db: &Arc<Mutex<Storage>>,
) -> Result<serde_json::Value, RpcError> {
    let params: GetBlockByHashParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {e}"),
        })?;

    let hash = parse_hex32(&params.hash, "hash")?;
    let idx = index.lock_or_recover();
    let height = idx.block_hashes.get(&hash).copied();
    drop(idx);

    match height {
        Some(h) => {
            let db_guard = db.lock_or_recover();
            let block =
                novai_consensus::ConsensusState::load_block(&*db_guard, h).map_err(|_| {
                    RpcError {
                        code: -32002,
                        message: "Failed to load block".to_string(),
                    }
                })?;
            drop(db_guard);
            match block {
                Some(b) => Ok(serde_json::to_value(BlockResult {
                    height: b.height,
                    round: b.round,
                    block_hash: hex::encode(hash),
                    parent_hash: hex::encode(b.parent_hash),
                    state_root: hex::encode(b.state_root),
                    tx_count: b.txs.len(),
                })
                .unwrap()),
                None => Ok(serde_json::Value::Null),
            }
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// Handle novai_getLatestBlock — return the latest committed block.
fn handle_get_latest_block(
    index: &Arc<Mutex<BlockchainIndex>>,
    db: &Arc<Mutex<Storage>>,
) -> Result<serde_json::Value, RpcError> {
    let height = index.lock_or_recover().committed_height;
    if height == 0 {
        return Ok(serde_json::Value::Null);
    }
    let db_guard = db.lock_or_recover();
    let block =
        novai_consensus::ConsensusState::load_block(&*db_guard, height).map_err(|_| RpcError {
            code: -32002,
            message: "Failed to load block".to_string(),
        })?;
    drop(db_guard);
    match block {
        Some(b) => {
            let hash = novai_consensus_types::codec::hash_block_v1(&b).map_err(|_| RpcError {
                code: -32002,
                message: "Failed to hash block".to_string(),
            })?;
            Ok(serde_json::to_value(BlockResult {
                height: b.height,
                round: b.round,
                block_hash: hex::encode(hash),
                parent_hash: hex::encode(b.parent_hash),
                state_root: hex::encode(b.state_root),
                tx_count: b.txs.len(),
            })
            .unwrap())
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// Check and enforce the per-IP sliding-window rate limit.
///
/// Returns `true` if the request from this IP should be rejected.
/// Periodically evicts stale IP entries (no requests in last 10s).
fn rpc_rate_limited(
    per_ip: &mut HashMap<IpAddr, VecDeque<Instant>>,
    ip: IpAddr,
    last_cleanup: &mut Instant,
) -> bool {
    let now = Instant::now();

    // Periodic cleanup: evict IPs with no recent activity (every 60s)
    if last_cleanup.elapsed() >= Duration::from_secs(60) {
        *last_cleanup = now;
        let stale_cutoff = now - Duration::from_secs(10);
        per_ip.retain(|_, timestamps| timestamps.back().is_some_and(|&t| t >= stale_cutoff));
    }

    let recent = per_ip.entry(ip).or_default();
    let one_sec_ago = now - Duration::from_secs(1);
    while recent.front().is_some_and(|&t| t < one_sec_ago) {
        recent.pop_front();
    }
    if recent.len() >= MAX_RPC_REQUESTS_PER_SEC {
        return true;
    }
    recent.push_back(now);
    false
}

/// Maximum RPC response body size (10MB). Prevents bandwidth exhaustion
/// from endpoints returning very large result sets.
const MAX_RPC_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Helper to create JSON response with security headers.
fn json_response<T: Serialize>(data: T) -> Response<std::io::Cursor<Vec<u8>>> {
    let json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());

    // M-02: Reject responses that exceed size limit
    if json.len() > MAX_RPC_RESPONSE_SIZE {
        let err = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {"code": -32003, "message": "Response too large"},
            "id": null,
        });
        let err_json = serde_json::to_string(&err).unwrap_or_else(|_| "{}".to_string());
        return Response::from_string(err_json)
            .with_header(
                "Content-Type: application/json"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            )
            .with_header(
                "Access-Control-Allow-Origin: null"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            );
    }

    // M-01: Restrictive CORS — deny cross-origin by default.
    // Use --rpc-bind behind a reverse proxy with custom CORS if web access needed.
    Response::from_string(json)
        .with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .unwrap(),
        )
        .with_header(
            "Access-Control-Allow-Origin: null"
                .parse::<tiny_http::Header>()
                .unwrap(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_request_deserializes() {
        let json =
            r#"{"jsonrpc":"2.0","method":"novai_submitTransaction","params":{"tx":"abcd"},"id":1}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "novai_submitTransaction");
    }

    #[test]
    fn test_rpc_error_response_serializes() {
        let response = RpcErrorResponse {
            jsonrpc: "2.0",
            error: RpcError {
                code: -32000,
                message: "Test error".to_string(),
            },
            id: serde_json::Value::Number(1.into()),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"code\":-32000"));
        assert!(json.contains("\"message\":\"Test error\""));
    }

    #[test]
    fn test_ai_entity_json_includes_v4_v5_fields() {
        let entity = AiEntityJson {
            id: "00".repeat(32),
            code_hash: "11".repeat(32),
            creator: "22".repeat(20),
            autonomy_mode: 0,
            capabilities: 0,
            economic_balance: "1000".to_string(),
            nonce: 5,
            pubkey: "33".repeat(32),
            memory_root: "44".repeat(32),
            params_root: "55".repeat(32),
            registered_at: 100,
            last_active_at: 200,
            is_active: true,
            reputation_score: 75,
            total_transactions: 42,
            reputation_events_count: 7,
            stake_balance: "5000000000000000000".to_string(),
            stake_locked_until: 12345,
            upgrade_count: 3,
            last_upgrade_height: 9000,
        };

        let json = serde_json::to_value(&entity).unwrap();

        assert_eq!(json["reputation_score"], 75);
        assert_eq!(json["total_transactions"], 42);
        assert_eq!(json["reputation_events_count"], 7);
        assert_eq!(json["stake_balance"], "5000000000000000000");
        assert_eq!(json["stake_locked_until"], 12345);
        assert_eq!(json["upgrade_count"], 3);
        assert_eq!(json["last_upgrade_height"], 9000);

        assert!(json["stake_balance"].is_string());
        assert!(json["economic_balance"].is_string());
        assert!(json["reputation_score"].is_number());
        assert!(json["total_transactions"].is_number());
        assert!(json["reputation_events_count"].is_number());
        assert!(json["stake_locked_until"].is_number());
        assert!(json["upgrade_count"].is_number());
        assert!(json["last_upgrade_height"].is_number());
    }

    // ========================================================================
    // Week 34 Phase 4 - novai_getUpgradeHistory wire shape
    // ========================================================================

    #[test]
    fn upgrade_record_json_serializes_hex_and_numbers() {
        let row = UpgradeRecordJson::from_record(UpgradeRecord {
            old_code_hash: [0x11u8; 32],
            new_code_hash: [0x22u8; 32],
            upgrade_height: 5000,
            upgrade_count: 2,
            reason_hash: [0x33u8; 32],
        });
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["old_code_hash"], "11".repeat(32));
        assert_eq!(json["new_code_hash"], "22".repeat(32));
        assert_eq!(json["reason_hash"], "33".repeat(32));
        assert_eq!(json["upgrade_height"], 5000);
        assert_eq!(json["upgrade_count"], 2);
    }

    #[test]
    fn get_upgrade_history_params_deserializes() {
        let json = serde_json::json!({
            "entity_id": "aa".repeat(32),
            "start_height": 0,
            "end_height": 10_000,
        });
        let params: GetUpgradeHistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.entity_id, "aa".repeat(32));
        assert_eq!(params.start_height, 0);
        assert_eq!(params.end_height, 10_000);
    }

    // ========================================================================
    // Week 28 Phase 4 - novai_getPaymentsByEntity wire shape
    // ========================================================================

    #[test]
    fn test_get_payments_by_entity_params_deserializes() {
        let json = serde_json::json!({
            "entity_id": "aa".repeat(32),
            "role": "payer",
            "start_height": 100u64,
            "end_height": 200u64,
        });
        let params: GetPaymentsByEntityParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.entity_id.len(), 64);
        assert_eq!(params.role, "payer");
        assert_eq!(params.start_height, 100);
        assert_eq!(params.end_height, 200);
    }

    #[test]
    fn test_payment_json_serialization_none_status() {
        // attested_status = NONE → JSON renders attested_status: null and
        // attested_height: null. Amount is a decimal string.
        let record = PaymentRecord {
            payer: [0xA1u8; 32],
            payee: [0xA2u8; 32],
            amount: 12_345,
            service_descriptor_hash: [0xA3u8; 32],
            request_hash: [0xA4u8; 32],
            payment_height: 500,
            max_block_height: 600,
            attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
            attested_height: 0,
        };
        let json = serde_json::to_value(PaymentJson::from(record)).unwrap();
        assert_eq!(json["payer"], "a1".repeat(32));
        assert_eq!(json["payee"], "a2".repeat(32));
        assert_eq!(json["amount"], "12345");
        assert_eq!(json["service_descriptor_hash"], "a3".repeat(32));
        assert_eq!(json["request_hash"], "a4".repeat(32));
        assert_eq!(json["payment_height"], 500);
        assert_eq!(json["max_block_height"], 600);
        assert!(json["attested_status"].is_null());
        assert!(json["attested_height"].is_null());
        assert!(json["amount"].is_string());
    }

    #[test]
    fn test_payment_json_serialization_attested_statuses() {
        let mk = |status: u8, attested_height: u64| PaymentRecord {
            payer: [0u8; 32],
            payee: [0u8; 32],
            amount: 1,
            service_descriptor_hash: [0u8; 32],
            request_hash: [0u8; 32],
            payment_height: 1,
            max_block_height: 2,
            attested_status: status,
            attested_height,
        };

        let delivered = serde_json::to_value(PaymentJson::from(mk(
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
            10,
        )))
        .unwrap();
        assert_eq!(delivered["attested_status"], "delivered");
        assert_eq!(delivered["attested_height"], 10);

        let failed =
            serde_json::to_value(PaymentJson::from(mk(PAYMENT_ATTESTATION_STATUS_FAILED, 20)))
                .unwrap();
        assert_eq!(failed["attested_status"], "failed");
        assert_eq!(failed["attested_height"], 20);

        // Corrupted-record fallback: an out-of-range status surfaces as
        // "unknown" rather than panicking. attested_height is still
        // emitted so operators can correlate it with the source record.
        let unknown = serde_json::to_value(PaymentJson::from(mk(0x7Fu8, 30))).unwrap();
        assert_eq!(unknown["attested_status"], "unknown");
        assert_eq!(unknown["attested_height"], 30);
    }

    // ========================================================================
    // Week 33 Phase 4 - novai_getPaymentsByEntity splits surface
    // ========================================================================

    fn sample_payment_record() -> PaymentRecord {
        PaymentRecord {
            payer: [0xA1u8; 32],
            payee: [0xA2u8; 32],
            amount: 10_000,
            service_descriptor_hash: [0xA3u8; 32],
            request_hash: [0xA4u8; 32],
            payment_height: 500,
            max_block_height: 600,
            attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
            attested_height: 0,
        }
    }

    #[test]
    fn test_payment_json_legacy_payment_renders_splits_null() {
        // Backward compat: a payment without an aux splits record
        // serialises with `"splits": null`. Week 28 RPC consumers see
        // every existing key unchanged.
        let json = serde_json::to_value(PaymentJson::from(sample_payment_record())).unwrap();
        assert!(
            json["splits"].is_null(),
            "legacy payments serialise splits as null",
        );
        // Spot-check that the original Week 28 keys are still present
        // with their original shapes.
        assert_eq!(json["amount"], "10000");
        assert!(json["amount"].is_string());
    }

    #[test]
    fn test_payment_json_with_splits_renders_array() {
        let record = sample_payment_record();
        let splits = PaymentSplitsRecord {
            entries: vec![
                PaymentSplitsRecordEntry {
                    recipient_entity_id: [0xA2u8; 32],
                    basis_points: 6_000,
                    credited_amount: 6_000,
                },
                PaymentSplitsRecordEntry {
                    recipient_entity_id: [0xB1u8; 32],
                    basis_points: 4_000,
                    credited_amount: 4_000,
                },
            ],
        };
        let json = serde_json::to_value(PaymentJson::from_record_with_splits(record, Some(splits)))
            .unwrap();
        // The PaymentRecord.payee is still the primary; the splits
        // array carries the per-recipient breakdown.
        assert_eq!(json["payee"], "a2".repeat(32));
        let splits_json = json["splits"].as_array().expect("splits is an array");
        assert_eq!(splits_json.len(), 2);
        assert_eq!(splits_json[0]["recipient_entity_id"], "a2".repeat(32));
        assert_eq!(splits_json[0]["basis_points"], 6_000);
        assert_eq!(splits_json[0]["credited_amount"], "6000");
        assert_eq!(splits_json[1]["recipient_entity_id"], "b1".repeat(32));
        assert_eq!(splits_json[1]["basis_points"], 4_000);
        assert_eq!(splits_json[1]["credited_amount"], "4000");
    }

    #[test]
    fn test_payment_json_splits_credited_amount_is_decimal_string() {
        // u64 amounts serialised as decimal strings (consistent with
        // PaymentJson.amount and the rest of the RPC surface).
        let record = sample_payment_record();
        let splits = PaymentSplitsRecord {
            entries: vec![
                PaymentSplitsRecordEntry {
                    recipient_entity_id: [0u8; 32],
                    basis_points: 5_000,
                    credited_amount: u64::MAX,
                },
                PaymentSplitsRecordEntry {
                    recipient_entity_id: [1u8; 32],
                    basis_points: 5_000,
                    credited_amount: 0,
                },
            ],
        };
        let json = serde_json::to_value(PaymentJson::from_record_with_splits(record, Some(splits)))
            .unwrap();
        assert_eq!(json["splits"][0]["credited_amount"], u64::MAX.to_string());
        assert!(json["splits"][0]["credited_amount"].is_string());
        assert_eq!(json["splits"][1]["credited_amount"], "0");
    }

    // ========================================================================
    // Week 36 Phase 4 - novai_getPaymentsByEntity condition surface
    // ========================================================================

    #[test]
    fn test_payment_json_legacy_payment_renders_condition_null() {
        // A payment with no condition aux record serialises with
        // `"condition": null` so Week 28/33 consumers see no shape change.
        let json = serde_json::to_value(PaymentJson::from(sample_payment_record())).unwrap();
        assert!(
            json["condition"].is_null(),
            "no condition serialises as null"
        );
    }

    #[test]
    fn test_payment_json_with_condition_renders_object() {
        let record = sample_payment_record();
        let condition = PaymentCondition::AnchorTagEquals {
            anchor_signal_hash: [0xC1u8; 32],
            expected_tag: b"price/ETH-USD".to_vec(),
        };
        let json = serde_json::to_value(PaymentJson::from_record_with_splits_and_condition(
            record,
            None,
            Some(condition),
        ))
        .unwrap();
        assert!(json["splits"].is_null(), "no splits when condition only");
        assert_eq!(json["condition"]["kind"], "anchor_tag_equals");
        assert_eq!(json["condition"]["anchor_signal_hash"], "c1".repeat(32));
        assert_eq!(json["condition"]["expected_tag"], "price/ETH-USD");
        assert_eq!(
            json["condition"]["expected_tag_hex"],
            hex::encode(b"price/ETH-USD")
        );
        assert!(json["condition"]["expected_data_hash"].is_null());
    }

    #[test]
    fn test_payment_json_condition_data_hash_equals_shape() {
        let condition = PaymentCondition::AnchorDataHashEquals {
            anchor_signal_hash: [0xC2u8; 32],
            expected_data_hash: [0xD4u8; 32],
        };
        let json = serde_json::to_value(PaymentJson::from_record_with_splits_and_condition(
            sample_payment_record(),
            None,
            Some(condition),
        ))
        .unwrap();
        assert_eq!(json["condition"]["kind"], "anchor_data_hash_equals");
        assert_eq!(json["condition"]["expected_data_hash"], "d4".repeat(32));
        assert!(json["condition"]["expected_tag"].is_null());
        assert!(json["condition"]["expected_tag_hex"].is_null());
    }

    // ========================================================================
    // Week 29 Phase 5 - novai_getServiceDescriptorsByCategory wire shape
    // ========================================================================

    fn sample_service_descriptor_pair() -> (MemoryObject, ServiceDescriptorData) {
        let owner = [0xA1u8; 32];
        let sd = ServiceDescriptorData {
            version: 1,
            service_name_hash: [0xB1u8; 32],
            service_url_hash: [0xB2u8; 32],
            description_hash: [0xB3u8; 32],
            category: SERVICE_CATEGORY_DATA_ORACLE,
            price_per_call: 0xDEAD_BEEFu64,
            subscription_rate_per_block: 42,
            min_reputation_score: 50,
            min_stake: 1_000_000_000_000_u128,
            capability_tags: 0x0F,
            status: SERVICE_STATUS_ACTIVE,
            reserved: [0u8; 7],
        };
        let data = sd.encode().to_vec();
        let obj = MemoryObject {
            object_id: [0xC1u8; 32],
            object_type: novai_ai_entities::MemoryObjectType::ServiceDescriptor,
            owner_entity: owner,
            created_at: 100,
            updated_at: 110,
            data,
        };
        (obj, sd)
    }

    #[test]
    fn test_get_service_descriptors_by_category_params_deserialize() {
        let json = serde_json::json!({ "category": 1u8 });
        let params: GetServiceDescriptorsByCategoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.category, SERVICE_CATEGORY_DATA_ORACLE);
    }

    #[test]
    fn test_service_descriptor_json_renders_amounts_as_strings_and_labels() {
        let (obj, sd) = sample_service_descriptor_pair();
        let json = serde_json::to_value(ServiceDescriptorJson::from((obj, sd))).unwrap();

        // Hex-encoded ids and hashes.
        assert_eq!(json["object_id"], "c1".repeat(32));
        assert_eq!(json["owner_entity"], "a1".repeat(32));
        assert_eq!(json["service_name_hash"], "b1".repeat(32));
        assert_eq!(json["service_url_hash"], "b2".repeat(32));
        assert_eq!(json["description_hash"], "b3".repeat(32));

        // Envelope timestamps preserved.
        assert_eq!(json["created_at"], 100);
        assert_eq!(json["updated_at"], 110);

        // Amounts as decimal strings.
        assert_eq!(json["price_per_call"], 0xDEAD_BEEFu64.to_string());
        assert_eq!(json["subscription_rate_per_block"], "42");
        assert_eq!(json["min_stake"], "1000000000000");
        assert!(json["price_per_call"].is_string());
        assert!(json["subscription_rate_per_block"].is_string());
        assert!(json["min_stake"].is_string());

        // Category and status get both numeric and label.
        assert_eq!(json["category"], SERVICE_CATEGORY_DATA_ORACLE);
        assert_eq!(json["category_label"], "data-oracle");
        assert_eq!(json["status"], SERVICE_STATUS_ACTIVE);
        assert_eq!(json["status_label"], "active");
    }

    #[test]
    fn test_service_category_label_covers_known_and_reserved_ranges() {
        assert_eq!(service_category_label(SERVICE_CATEGORY_GENERIC), "generic");
        assert_eq!(
            service_category_label(SERVICE_CATEGORY_DATA_ORACLE),
            "data-oracle"
        );
        assert_eq!(
            service_category_label(SERVICE_CATEGORY_INFERENCE),
            "inference"
        );
        assert_eq!(service_category_label(SERVICE_CATEGORY_COMPUTE), "compute");
        assert_eq!(service_category_label(SERVICE_CATEGORY_STORAGE), "storage");
        assert_eq!(service_category_label(SERVICE_CATEGORY_INDEXER), "indexer");
        assert_eq!(
            service_category_label(SERVICE_CATEGORY_SIGNAL_PROVIDER),
            "signal-provider"
        );
        assert_eq!(
            service_category_label(SERVICE_CATEGORY_VERIFICATION),
            "verification"
        );
        assert_eq!(
            service_category_label(SERVICE_CATEGORY_MONITORING),
            "monitoring"
        );
        assert_eq!(service_category_label(SERVICE_CATEGORY_GATEWAY), "gateway");

        // 10..=15 fall through to "reserved" (no v1 well-known name yet).
        assert_eq!(service_category_label(10), "reserved");
        assert_eq!(
            service_category_label(SERVICE_CATEGORY_RESERVED_MAX),
            "reserved"
        );

        // 16 and above are the governance-allocation range.
        assert_eq!(service_category_label(16), "governance");
        assert_eq!(service_category_label(255), "governance");
    }

    #[test]
    fn test_service_status_label_paused_deprecated_and_unknown() {
        assert_eq!(service_status_label(SERVICE_STATUS_ACTIVE), "active");
        assert_eq!(service_status_label(SERVICE_STATUS_PAUSED), "paused");
        assert_eq!(
            service_status_label(SERVICE_STATUS_DEPRECATED),
            "deprecated"
        );
        // Corrupted-record fallback.
        assert_eq!(service_status_label(99), "unknown");
    }

    // ========================================================================
    // Week 30 Phase 3 - novai_getVkRegistration / novai_listVkRegistrations
    // ========================================================================

    fn sample_vk_registration_pair() -> (MemoryObject, VkRegistrationData) {
        let owner = [0xA1u8; 32];
        let reg = VkRegistrationData {
            version: 1,
            proof_type: PROOF_TYPE_GROTH16,
            code_hash: [0xC0u8; 32],
            label: b"sum-v1".to_vec(),
            vk_bytes: (0..16u8).collect(),
        };
        let data = reg.encode();
        let obj = MemoryObject {
            object_id: [0xD1u8; 32],
            object_type: novai_ai_entities::MemoryObjectType::VkRegistration,
            owner_entity: owner,
            created_at: 200,
            updated_at: 210,
            data,
        };
        (obj, reg)
    }

    #[test]
    fn test_get_vk_registration_params_deserialize() {
        let json = serde_json::json!({ "id": "d1".repeat(32) });
        let params: GetVkRegistrationParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "d1".repeat(32));
    }

    #[test]
    fn test_list_vk_registrations_params_deserialize() {
        let json = serde_json::json!({ "entity_id": "a1".repeat(32) });
        let params: ListVkRegistrationsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.entity_id, "a1".repeat(32));
    }

    #[test]
    fn test_vk_registration_json_layout() {
        let (obj, reg) = sample_vk_registration_pair();
        let json = serde_json::to_value(VkRegistrationJson::from((obj, reg))).unwrap();

        assert_eq!(json["object_id"], "d1".repeat(32));
        assert_eq!(json["owner_entity"], "a1".repeat(32));
        assert_eq!(json["created_at"], 200);
        assert_eq!(json["updated_at"], 210);
        assert_eq!(json["version"], 1);
        assert_eq!(json["proof_type"], PROOF_TYPE_GROTH16);
        assert_eq!(json["proof_type_label"], "groth16");
        assert_eq!(json["code_hash"], "c0".repeat(32));
        assert_eq!(json["label"], "sum-v1");
        assert_eq!(json["vk_len"], 16);
        // Compressed VK bytes round-trip as hex.
        let expected_hex: String = (0..16u8).map(|b| format!("{b:02x}")).collect();
        assert_eq!(json["vk_bytes_hex"], expected_hex);
    }

    #[test]
    fn test_proof_type_label_covers_known_and_unknown() {
        assert_eq!(proof_type_label(PROOF_TYPE_STUB), "stub");
        assert_eq!(proof_type_label(PROOF_TYPE_GROTH16), "groth16");
        assert_eq!(proof_type_label(PROOF_TYPE_PLONK), "plonk");
        assert_eq!(
            proof_type_label(PROOF_TYPE_GROTH16_REGISTERED),
            "groth16-registered"
        );
        assert_eq!(
            proof_type_label(PROOF_TYPE_PLONK_REGISTERED),
            "plonk-registered"
        );
        assert_eq!(proof_type_label(99), "unknown");
    }

    // ========================================================================
    // Week 35 Phase 4 - oracle anchor RPC wire shapes
    // ========================================================================

    #[test]
    fn oracle_anchor_json_serializes_hex_and_tag() {
        let row = OracleAnchorJson::from_record(OracleAnchorRecord {
            issuer_entity_id: [0x11u8; 32],
            data_hash: [0x22u8; 32],
            external_timestamp: 1_700_000_000,
            source_hash: [0x33u8; 32],
            expiry_height: 5000,
            anchor_height: 900,
            data_tag: b"price/ETH-USD".to_vec(),
        });
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["issuer_entity_id"], "11".repeat(32));
        assert_eq!(json["data_hash"], "22".repeat(32));
        assert_eq!(json["source_hash"], "33".repeat(32));
        assert_eq!(json["external_timestamp"], 1_700_000_000u64);
        assert_eq!(json["expiry_height"], 5000);
        assert_eq!(json["anchor_height"], 900);
        assert_eq!(json["data_tag"], "price/ETH-USD");
        assert_eq!(json["data_tag_hex"], hex::encode(b"price/ETH-USD"));
    }

    #[test]
    fn oracle_anchor_json_handles_non_utf8_tag() {
        let row = OracleAnchorJson::from_record(OracleAnchorRecord {
            issuer_entity_id: [0u8; 32],
            data_hash: [1u8; 32],
            external_timestamp: 1,
            source_hash: [0u8; 32],
            expiry_height: 0,
            anchor_height: 1,
            data_tag: vec![0xFF, 0xFE], // not valid UTF-8
        });
        // Lossy UTF-8 view must not panic; the hex view is exact.
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["data_tag_hex"], "fffe");
    }

    #[test]
    fn get_oracle_anchors_by_entity_params_deserialize() {
        let json = serde_json::json!({
            "entity_id": "aa".repeat(32),
            "start_height": 0,
            "end_height": 10_000,
        });
        let p: GetOracleAnchorsByEntityParams = serde_json::from_value(json).unwrap();
        assert_eq!(p.entity_id, "aa".repeat(32));
        assert_eq!(p.end_height, 10_000);
        assert!(p.ts_min.is_none());
        assert!(p.ts_max.is_none());

        let with_ts = serde_json::json!({
            "entity_id": "aa".repeat(32),
            "start_height": 100,
            "end_height": 200,
            "ts_min": 1_000,
            "ts_max": 2_000,
        });
        let p2: GetOracleAnchorsByEntityParams = serde_json::from_value(with_ts).unwrap();
        assert_eq!(p2.ts_min, Some(1_000));
        assert_eq!(p2.ts_max, Some(2_000));
    }

    #[test]
    fn get_oracle_anchors_by_tag_params_deserialize() {
        let json = serde_json::json!({
            "data_tag": "price/ETH-USD",
            "start_height": 0,
            "end_height": 500,
        });
        let p: GetOracleAnchorsByTagParams = serde_json::from_value(json).unwrap();
        assert_eq!(p.data_tag, "price/ETH-USD");
        assert_eq!(p.end_height, 500);
    }

    #[test]
    fn get_oracle_anchor_params_deserialize() {
        let json = serde_json::json!({ "signal_hash": "10".repeat(32) });
        let p: GetOracleAnchorParams = serde_json::from_value(json).unwrap();
        assert_eq!(p.signal_hash, "10".repeat(32));
    }

    #[test]
    fn oracle_ts_in_range_filters_correctly() {
        assert!(oracle_ts_in_range(100, None, None));
        assert!(oracle_ts_in_range(100, Some(100), Some(100)));
        assert!(oracle_ts_in_range(150, Some(100), Some(200)));
        assert!(!oracle_ts_in_range(99, Some(100), None));
        assert!(!oracle_ts_in_range(201, None, Some(200)));
    }

    // ========================================================================
    // Public HTTP faucet (GET /faucet/<address>)
    // ========================================================================

    #[test]
    fn parse_public_faucet_path_valid_lowercase() {
        let url = format!("/faucet/{}", "ab".repeat(32));
        let addr = parse_public_faucet_path(&url).expect("valid address parses");
        assert_eq!(addr, [0xABu8; 32]);
    }

    #[test]
    fn parse_public_faucet_path_valid_uppercase() {
        let url = format!("/faucet/{}", "CD".repeat(32));
        let addr = parse_public_faucet_path(&url).expect("uppercase hex parses");
        assert_eq!(addr, [0xCDu8; 32]);
    }

    #[test]
    fn parse_public_faucet_path_accepts_trailing_slash() {
        let url = format!("/faucet/{}/", "11".repeat(32));
        let addr = parse_public_faucet_path(&url).expect("trailing slash is tolerated");
        assert_eq!(addr, [0x11u8; 32]);
    }

    #[test]
    fn parse_public_faucet_path_rejects_short_address() {
        let url = format!("/faucet/{}", "ab".repeat(16)); // 32 chars, want 64
        let (status, msg) = parse_public_faucet_path(&url).unwrap_err();
        assert_eq!(status, 400);
        assert!(msg.contains("64 hex chars"), "msg was {msg}");
    }

    #[test]
    fn parse_public_faucet_path_rejects_long_address() {
        let url = format!("/faucet/{}", "ab".repeat(40));
        let (status, _) = parse_public_faucet_path(&url).unwrap_err();
        assert_eq!(status, 400);
    }

    #[test]
    fn parse_public_faucet_path_rejects_non_hex() {
        let url = format!("/faucet/{}", "zz".repeat(32));
        let (status, msg) = parse_public_faucet_path(&url).unwrap_err();
        assert_eq!(status, 400);
        assert!(msg.contains("invalid address hex"), "msg was {msg}");
    }

    #[test]
    fn parse_public_faucet_path_rejects_wrong_prefix() {
        let url = format!("/feucet/{}", "ab".repeat(32));
        let (status, _) = parse_public_faucet_path(&url).unwrap_err();
        assert_eq!(status, 404);
    }

    #[test]
    fn public_faucet_cooldown_no_prior_dispense_is_none() {
        let now_secs: u64 = 1_700_000_000;
        assert!(public_faucet_cooldown_remaining(None, now_secs).is_none());
    }

    #[test]
    fn public_faucet_cooldown_just_dispensed_is_within_window() {
        let last: u64 = 1_700_000_000;
        // Same second: zero elapsed, so the full cooldown window is remaining.
        let remaining = public_faucet_cooldown_remaining(Some(last), last)
            .expect("just-dispensed IP must be cooling down");
        assert!(remaining > 0);
        assert!(remaining <= PUBLIC_FAUCET_PER_IP_COOLDOWN_SECS);
    }

    #[test]
    fn public_faucet_cooldown_after_window_is_none() {
        let last: u64 = 1_700_000_000;
        let after = last + PUBLIC_FAUCET_PER_IP_COOLDOWN_SECS + 1;
        assert!(public_faucet_cooldown_remaining(Some(last), after).is_none());
    }

    #[test]
    fn public_faucet_cooldown_clock_skew_backwards_treated_as_zero_elapsed() {
        // If a stored timestamp is somehow in the future relative to "now"
        // (clock skew between persist and reload), saturating_sub yields
        // zero elapsed and the full cooldown is still in effect.
        let last: u64 = 1_700_000_500;
        let now: u64 = 1_700_000_000;
        let remaining = public_faucet_cooldown_remaining(Some(last), now)
            .expect("future-stamped IP must still be cooling down");
        assert_eq!(remaining, PUBLIC_FAUCET_PER_IP_COOLDOWN_SECS);
    }

    #[test]
    fn public_faucet_error_body_omits_retry_after_when_none() {
        let body = public_faucet_error_body("nope", None);
        assert!(body.contains("\"error\":\"nope\""));
        assert!(!body.contains("retry_after_secs"));
    }

    #[test]
    fn public_faucet_error_body_includes_retry_after_when_some() {
        let body = public_faucet_error_body("rate-limited", Some(42));
        assert!(body.contains("\"error\":\"rate-limited\""));
        assert!(body.contains("\"retry_after_secs\":42"));
    }

    // ========================================================================
    // CIDR matcher (faucet trusted-proxy allowlist)
    // ========================================================================

    fn ipv4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn ipv6(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn cidr_parse_ipv4_24_contains_and_excludes() {
        let cidr = CidrBlock::parse("10.0.0.0/24").expect("/24 parses");
        assert!(cidr.contains(ipv4("10.0.0.0")));
        assert!(cidr.contains(ipv4("10.0.0.5")));
        assert!(cidr.contains(ipv4("10.0.0.255")));
        // Third octet differs: the /24 mask must not over-match inside its
        // own /16.
        assert!(!cidr.contains(ipv4("10.0.1.0")));
        // First octet differs. RFC 5737 documentation space, reserved by the
        // IETF for exactly this and never routable, so no real address
        // appears in the tree.
        assert!(!cidr.contains(ipv4("203.0.113.0")));
    }

    #[test]
    fn cidr_parse_ipv6_64_contains_and_excludes() {
        let cidr = CidrBlock::parse("2001:db8::/64").expect("/64 parses");
        assert!(cidr.contains(ipv6("2001:db8::1")));
        assert!(cidr.contains(ipv6("2001:db8::ffff:ffff")));
        assert!(!cidr.contains(ipv6("2001:db9::1")));
    }

    #[test]
    fn cidr_parse_zero_prefix_contains_everything() {
        let v4 = CidrBlock::parse("0.0.0.0/0").expect("/0 parses");
        assert!(v4.contains(ipv4("1.2.3.4")));
        assert!(v4.contains(ipv4("255.255.255.255")));
        let v6 = CidrBlock::parse("::/0").expect("/0 v6 parses");
        assert!(v6.contains(ipv6("::1")));
        assert!(v6.contains(ipv6("2001:db8::1")));
    }

    #[test]
    fn cidr_parse_invalid_prefix_too_large() {
        assert!(CidrBlock::parse("10.0.0.0/33").is_err());
        assert!(CidrBlock::parse("::/129").is_err());
    }

    #[test]
    fn cidr_parse_malformed_inputs_rejected() {
        assert!(CidrBlock::parse("garbage").is_err());
        assert!(CidrBlock::parse("10.0.0.0").is_err());
        assert!(CidrBlock::parse("10.0.0.0/").is_err());
        assert!(CidrBlock::parse("/24").is_err());
        assert!(CidrBlock::parse("10.0.0.0/abc").is_err());
        assert!(CidrBlock::parse("999.0.0.0/8").is_err());
    }

    #[test]
    fn cidr_v4_does_not_match_v6_and_vice_versa() {
        let v4 = CidrBlock::parse("0.0.0.0/0").unwrap();
        assert!(!v4.contains(ipv6("::1")));
        let v6 = CidrBlock::parse("::/0").unwrap();
        assert!(!v6.contains(ipv4("1.2.3.4")));
    }

    // ========================================================================
    // resolve_client_ip (X-Forwarded-For walker)
    // ========================================================================

    fn xff_header(value: &str) -> tiny_http::Header {
        format!("X-Forwarded-For: {value}")
            .parse::<tiny_http::Header>()
            .expect("XFF header parses")
    }

    fn cidrs(specs: &[&str]) -> Vec<CidrBlock> {
        specs
            .iter()
            .map(|s| CidrBlock::parse(s).expect("test CIDR parses"))
            .collect()
    }

    #[test]
    fn resolve_no_xff_no_allowlist_returns_peer() {
        let peer = ipv4("1.2.3.4");
        let out = resolve_client_ip(peer, &[], &[]);
        assert_eq!(out, peer);
    }

    #[test]
    fn resolve_xff_present_but_no_allowlist_ignores_xff() {
        // Safe default: even if a client sends XFF, without a configured
        // trusted-proxy allowlist NOVAI must not honor it.
        let peer = ipv4("1.2.3.4");
        let headers = [xff_header("5.6.7.8")];
        let out = resolve_client_ip(peer, &headers, &[]);
        assert_eq!(out, peer);
    }

    #[test]
    fn resolve_xff_peer_not_in_allowlist_ignores_xff() {
        let peer = ipv4("1.2.3.4");
        let headers = [xff_header("5.6.7.8")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, peer);
    }

    #[test]
    fn resolve_xff_single_hop_in_allowlist_returns_xff_client() {
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("203.0.113.5")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv4("203.0.113.5"));
    }

    #[test]
    fn resolve_xff_multi_hop_walks_rightward_to_first_untrusted() {
        // Chain order: client, hop1, hop2. Peer is the rightmost-implied hop.
        // hop2 (10.0.0.2) is trusted, client (1.2.3.4) is not. NOVAI must
        // return the first untrusted entry walking from the right.
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("1.2.3.4, 10.0.0.2")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv4("1.2.3.4"));
    }

    #[test]
    fn resolve_xff_all_entries_trusted_returns_leftmost() {
        // If every XFF entry is itself in the trusted-proxy allowlist, no
        // untrusted hop exists. NOVAI returns the leftmost entry, which is
        // the original sender from inside the trust boundary.
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("10.0.0.3, 10.0.0.2")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv4("10.0.0.3"));
    }

    #[test]
    fn resolve_xff_malformed_entry_is_skipped() {
        // Garbage entries are dropped during parse. The walk then proceeds
        // over the remaining well-formed entries.
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("not-an-ip, 1.2.3.4")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv4("1.2.3.4"));
    }

    #[test]
    fn resolve_xff_with_whitespace_is_trimmed() {
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("  1.2.3.4  ,  10.0.0.2  ")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv4("1.2.3.4"));
    }

    #[test]
    fn resolve_xff_ipv6_in_chain_returns_ipv6_client() {
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("2001:db8::1")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv6("2001:db8::1"));
    }

    #[test]
    fn resolve_xff_empty_header_value_falls_back_to_peer() {
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, peer);
    }

    #[test]
    fn resolve_xff_whitespace_only_falls_back_to_peer() {
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("   ")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, peer);
    }

    #[test]
    fn resolve_xff_header_name_is_case_insensitive() {
        // tiny_http stores HeaderField with case-insensitive equiv(); confirm
        // the find_xff_header helper actually matches a lowercase header.
        let peer = ipv4("10.0.0.1");
        let h = "x-forwarded-for: 1.2.3.4"
            .parse::<tiny_http::Header>()
            .expect("lowercase header parses");
        let out = resolve_client_ip(peer, &[h], &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, ipv4("1.2.3.4"));
    }

    #[test]
    fn resolve_xff_only_malformed_entries_falls_back_to_peer() {
        let peer = ipv4("10.0.0.1");
        let headers = [xff_header("not-an-ip, also-not")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8"]));
        assert_eq!(out, peer);
    }

    #[test]
    fn resolve_multiple_trusted_cidrs_v4_and_v6() {
        // Confirm the allowlist supports multiple CIDR blocks of mixed families.
        let peer = ipv6("2001:db8::1");
        let headers = [xff_header("203.0.113.5")];
        let out = resolve_client_ip(peer, &headers, &cidrs(&["10.0.0.0/8", "2001:db8::/32"]));
        assert_eq!(out, ipv4("203.0.113.5"));
    }
}
