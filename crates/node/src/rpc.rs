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
use crate::MutexExt;
use mempool::{NonceProvider, TxMempool};
use novai_ai_entities::{
    AiSignalType, MemoryObject, ServiceDescriptorData, SignalCommitment, SERVICE_CATEGORY_COMPUTE,
    SERVICE_CATEGORY_DATA_ORACLE, SERVICE_CATEGORY_GATEWAY, SERVICE_CATEGORY_GENERIC,
    SERVICE_CATEGORY_INDEXER, SERVICE_CATEGORY_INFERENCE, SERVICE_CATEGORY_MONITORING,
    SERVICE_CATEGORY_RESERVED_MAX, SERVICE_CATEGORY_SIGNAL_PROVIDER, SERVICE_CATEGORY_STORAGE,
    SERVICE_CATEGORY_VERIFICATION, SERVICE_STATUS_ACTIVE, SERVICE_STATUS_DEPRECATED,
    SERVICE_STATUS_PAUSED,
};
use novai_codec::{decode_tx_v1_signed, txid_v1};
use novai_consensus_types;
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_execution::{
    get_memory_objects_by_entity, get_payments_by_entity, get_service_descriptors_by_category,
    get_signals_by_height, get_signals_by_issuer, get_signals_by_type, read_account_or_default,
    read_ai_entity, PaymentRecord, PaymentRole, PAYMENT_ATTESTATION_STATUS_DELIVERED,
    PAYMENT_ATTESTATION_STATUS_FAILED, PAYMENT_ATTESTATION_STATUS_NONE,
};
use novai_p2p::{NetworkMessage, PeerManager};
use novai_types::{Address, TxV1, TxVersion};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
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

/// JSON-serializable PaymentRecord. Amount is rendered as a decimal
/// string (consistent with the rest of the RPC surface where u64/u128
/// values are returned as strings to avoid JSON precision loss);
/// attested_status is rendered as a human-readable label.
#[derive(Debug, Serialize)]
struct PaymentJson {
    /// Hex-encoded 32-byte payer entity id.
    payer: String,
    /// Hex-encoded 32-byte payee entity id.
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
        }
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
}

/// Result for novai_getAiEntity.
#[derive(Debug, Serialize)]
struct GetAiEntityResult {
    entity: Option<AiEntityJson>,
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

/// Start the JSON-RPC server.
///
/// Spawns a dedicated thread to handle HTTP JSON-RPC requests.
/// Returns immediately after starting the listener.
///
/// # Arguments
/// - `bind_addr` - Address to bind the HTTP server (e.g., "0.0.0.0:9545")
/// - `mempool` - Shared mempool for transaction submission
/// - `nonce_provider` - Provides expected nonces for transaction validation
///
/// # Errors
/// Returns error if the server cannot bind to the address (e.g., port in use).
pub fn start_rpc_server(
    bind_addr: &str,
    mempool: Arc<Mutex<TxMempool>>,
    nonce_provider: Arc<dyn NonceProvider + Send + Sync>,
    peer_manager: Option<Arc<PeerManager>>,
) -> Result<(), String> {
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start RPC server: {e}"))?;

    tracing::info!(%addr, "RPC server listening");

    thread::spawn(move || {
        let mut per_ip_limits: HashMap<IpAddr, VecDeque<Instant>> = HashMap::new();
        let mut last_cleanup = Instant::now();
        let nonce = SharedNonceProvider(nonce_provider);

        for mut request in server.incoming_requests() {
            // Per-IP rate limiting: sliding 1-second window
            let client_ip = request
                .remote_addr()
                .map(std::net::SocketAddr::ip)
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            if rpc_rate_limited(&mut per_ip_limits, client_ip, &mut last_cleanup) {
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
///
/// # Errors
/// Returns error if the server cannot bind to the address.
pub fn start_rpc_server_with_state(
    bind_addr: &str,
    mempool: Arc<Mutex<TxMempool>>,
    nonce_provider: Arc<dyn NonceProvider + Send + Sync>,
    db: Arc<Mutex<Storage>>,
    dev_keys: bool,
    blockchain_index: Arc<Mutex<BlockchainIndex>>,
    faucet_key: Option<ed25519_dalek::SigningKey>,
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

            // Per-IP rate limiting: sliding 1-second window
            let client_ip = request
                .remote_addr()
                .map(std::net::SocketAddr::ip)
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            if rpc_rate_limited(&mut per_ip_limits, client_ip, &mut last_cleanup) {
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
                    match handle_submit_tx(&rpc_request, &mempool, &nonce, &None) {
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
            // Map mempool errors to distinct codes so clients can distinguish
            // rejection types without leaking internal debug details (H-06).
            // Codes: -32001 = MempoolFull, -32010 = NonceTooLow,
            //        -32011 = FeeTooLow, -32012 = SenderLimitExceeded,
            //        -32013 = other validation error
            let (code, message) = match e {
                mempool::TxMempoolError::MempoolFull { .. } => (-32001, "MempoolFull".to_string()),
                mempool::TxMempoolError::NonceTooLow { expected, got } => (
                    -32010,
                    format!("NonceTooLow: expected {expected}, got {got}"),
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
    let payments = get_payments_by_entity(
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
        payments: payments.into_iter().map(PaymentJson::from).collect(),
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

    Ok(GetAiEntityResult {
        entity: entity.map(|e| AiEntityJson {
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
        }),
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

    let faucet_pk = faucet_sk.verifying_key();
    let faucet_addr = address_from_pubkey(&faucet_pk);

    // Get current nonce for faucet account
    let nonce = nonce_provider.expected_nonce(&faucet_addr);

    // Build transfer payload: [version:1][to:32][amount:8 BE]
    let mut payload = Vec::with_capacity(41);
    payload.push(1); // Transfer payload version
    payload.extend_from_slice(&to_address);
    payload.extend_from_slice(&FAUCET_AMOUNT.to_be_bytes());

    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: faucet_addr,
        pubkey: faucet_pk.to_bytes(),
        nonce,
        fee: 100, // MIN_FEE_TRANSFER
        payload,
        sig: [0u8; 64],
    };

    sign_tx_v1(&faucet_sk, &mut tx).map_err(|_| RpcError {
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

    // Submit to mempool
    let mut mempool_guard = mempool.lock_or_recover();
    mempool_guard
        .insert(tx, nonce_provider)
        .map_err(|_| RpcError {
            code: -32001,
            message: "Faucet transaction rejected by mempool".to_string(),
        })?;
    drop(mempool_guard);

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
        };

        let json = serde_json::to_value(&entity).unwrap();

        assert_eq!(json["reputation_score"], 75);
        assert_eq!(json["total_transactions"], 42);
        assert_eq!(json["reputation_events_count"], 7);
        assert_eq!(json["stake_balance"], "5000000000000000000");
        assert_eq!(json["stake_locked_until"], 12345);

        assert!(json["stake_balance"].is_string());
        assert!(json["economic_balance"].is_string());
        assert!(json["reputation_score"].is_number());
        assert!(json["total_transactions"].is_number());
        assert!(json["reputation_events_count"].is_number());
        assert!(json["stake_locked_until"].is_number());
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
}
