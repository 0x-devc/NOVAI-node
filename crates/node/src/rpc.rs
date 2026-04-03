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
use mempool::{NonceProvider, TxMempool};
use novai_ai_entities::{AiSignalType, SignalCommitment};
use novai_codec::{decode_tx_v1_signed, txid_v1};
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_execution::{
    get_memory_objects_by_entity, get_signals_by_height, get_signals_by_issuer,
    get_signals_by_type, read_account_or_default, read_ai_entity,
};
use novai_p2p::{NetworkMessage, PeerManager};
use novai_types::{Address, TxV1, TxVersion};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Response, Server, StatusCode};

/// Maximum RPC requests per second before rate limiting kicks in.
const MAX_RPC_REQUESTS_PER_SEC: usize = 100;

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
        let mut recent_requests: VecDeque<Instant> = VecDeque::new();
        let nonce = SharedNonceProvider(nonce_provider);

        for mut request in server.incoming_requests() {
            // Rate limiting: sliding 1-second window
            if rpc_rate_limited(&mut recent_requests) {
                let _ = request.respond(
                    Response::from_string("Too Many Requests").with_status_code(StatusCode(429)),
                );
                continue;
            }
            // Read request body
            let mut body = String::new();
            if let Err(e) = request.as_reader().read_to_string(&mut body) {
                let _ = request.respond(
                    Response::from_string(format!("Failed to read request: {}", e))
                        .with_status_code(StatusCode(400)),
                );
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
                            message: format!("Parse error: {}", e),
                        },
                        id: serde_json::Value::Null,
                    };
                    let _ = request.respond(json_response(error_response));
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
                let _ = request.respond(json_response(error_response));
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

            let _ = request.respond(http_response);
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
) -> Result<(), String> {
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start RPC server: {e}"))?;

    tracing::info!(%addr, "RPC server listening (with state queries)");

    thread::spawn(move || {
        let mut recent_requests: VecDeque<Instant> = VecDeque::new();
        let nonce = SharedNonceProvider(nonce_provider);

        for mut request in server.incoming_requests() {
            // Rate limiting: sliding 1-second window
            if rpc_rate_limited(&mut recent_requests) {
                let _ = request.respond(
                    Response::from_string("Too Many Requests").with_status_code(StatusCode(429)),
                );
                continue;
            }
            // Read request body
            let mut body = String::new();
            if let Err(e) = request.as_reader().read_to_string(&mut body) {
                let _ = request.respond(
                    Response::from_string(format!("Failed to read request: {}", e))
                        .with_status_code(StatusCode(400)),
                );
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
                            message: format!("Parse error: {}", e),
                        },
                        id: serde_json::Value::Null,
                    };
                    let _ = request.respond(json_response(error_response));
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
                let _ = request.respond(json_response(error_response));
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
                "novai_faucet" => match handle_faucet(&rpc_request, &mempool, &nonce, dev_keys) {
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

            let _ = request.respond(http_response);
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
            message: format!("Invalid params: {}", e),
        })?;

    // Decode hex transaction
    let tx_bytes = hex::decode(&params.tx).map_err(|e| RpcError {
        code: -32000,
        message: format!("Invalid hex encoding: {}", e),
    })?;

    // Decode transaction
    let tx: TxV1 = decode_tx_v1_signed(&tx_bytes).map_err(|e| RpcError {
        code: -32000,
        message: format!("Invalid transaction encoding: {:?}", e),
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
    let txid = txid_v1(&tx).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to compute txid: {:?}", e),
    })?;

    // Submit to mempool
    let mut mempool_guard = mempool.lock().unwrap();
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
            Err(RpcError {
                code: -32001,
                message: format!("Mempool rejected transaction: {:?}", e),
            })
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
            message: format!("Invalid params: {}", e),
        })?;

    let addr_bytes = hex::decode(&params.address).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address hex: {}", e),
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
            message: format!("Invalid params: {}", e),
        })?;

    let db = db.lock().unwrap();
    let signals = get_signals_by_height(&*db, params.height).map_err(|e| RpcError {
        code: -32002,
        message: format!("State query error: {:?}", e),
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
            message: format!("Invalid params: {}", e),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {} heights per query",
                MAX_SIGNAL_QUERY_RANGE
            ),
        });
    }

    // Decode issuer hex
    let issuer_bytes = hex::decode(&params.issuer).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid issuer hex: {}", e),
    })?;
    if issuer_bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("Issuer must be 32 bytes, got {}", issuer_bytes.len()),
        });
    }
    let mut issuer = [0u8; 32];
    issuer.copy_from_slice(&issuer_bytes);

    let db = db.lock().unwrap();
    let signals = get_signals_by_issuer(&*db, &issuer, params.start_height, params.end_height)
        .map_err(|e| RpcError {
            code: -32002,
            message: format!("State query error: {:?}", e),
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
            message: format!("Invalid params: {}", e),
        })?;

    if params.end_height.saturating_sub(params.start_height) > MAX_SIGNAL_QUERY_RANGE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Height range too large: max {} heights per query",
                MAX_SIGNAL_QUERY_RANGE
            ),
        });
    }

    // Validate signal type
    let signal_type = AiSignalType::from_byte(params.signal_type).ok_or_else(|| RpcError {
        code: -32602,
        message: format!("Invalid signal type: {} (must be 0-6)", params.signal_type),
    })?;

    let db = db.lock().unwrap();
    let signals = get_signals_by_type(&*db, signal_type, params.start_height, params.end_height)
        .map_err(|e| RpcError {
            code: -32002,
            message: format!("State query error: {:?}", e),
        })?;

    Ok(SignalQueryResult {
        signals: signals
            .into_iter()
            .map(SignalCommitmentJson::from)
            .collect(),
    })
}

// ============================================================================
// STATE QUERY HANDLERS (CLI support)
// ============================================================================

/// Parse and validate a 32-byte hex-encoded value.
fn parse_hex32(hex_str: &str, field_name: &str) -> Result<[u8; 32], RpcError> {
    let bytes = hex::decode(hex_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid {} hex: {}", field_name, e),
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
            message: format!("Invalid params: {}", e),
        })?;

    let address = parse_hex32(&params.address, "address")?;
    let db = db.lock().unwrap();
    let account = read_account_or_default(&*db, &address).map_err(|e| RpcError {
        code: -32002,
        message: format!("State query error: {:?}", e),
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
            message: format!("Invalid params: {}", e),
        })?;

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    let db = db.lock().unwrap();
    let entity = read_ai_entity(&*db, &entity_id).map_err(|e| RpcError {
        code: -32002,
        message: format!("State query error: {:?}", e),
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
            message: format!("Invalid params: {}", e),
        })?;

    let entity_id = parse_hex32(&params.entity_id, "entity_id")?;
    let db = db.lock().unwrap();
    let objects = get_memory_objects_by_entity(&*db, &entity_id).map_err(|e| RpcError {
        code: -32002,
        message: format!("State query error: {:?}", e),
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
fn handle_faucet(
    request: &RpcRequest,
    mempool: &Arc<Mutex<TxMempool>>,
    nonce_provider: &SharedNonceProvider,
    dev_keys: bool,
) -> Result<FaucetResult, RpcError> {
    if !dev_keys {
        return Err(RpcError {
            code: -32000,
            message: "Faucet is only available in dev mode (--dev-keys)".to_string(),
        });
    }

    let params: FaucetParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {}", e),
        })?;

    let to_address = parse_hex32(&params.address, "address")?;

    // Derive faucet signing key from dev account index 99
    let seed_byte = (FAUCET_ACCOUNT_INDEX % 256) as u8;
    let mut seed = [seed_byte; 32];
    let index_bytes = FAUCET_ACCOUNT_INDEX.to_le_bytes();
    for (j, &b) in index_bytes.iter().enumerate() {
        seed[j] ^= b;
    }
    let faucet_sk = ed25519_dalek::SigningKey::from_bytes(&seed);
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

    sign_tx_v1(&faucet_sk, &mut tx).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to sign faucet tx: {:?}", e),
    })?;

    let txid = txid_v1(&tx).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to compute txid: {:?}", e),
    })?;

    // Submit to mempool
    let mut mempool_guard = mempool.lock().unwrap();
    mempool_guard
        .insert(tx, nonce_provider)
        .map_err(|e| RpcError {
            code: -32001,
            message: format!("Faucet tx rejected by mempool: {:?}", e),
        })?;
    drop(mempool_guard);

    tracing::info!(
        to = %hex::encode(to_address),
        amount = FAUCET_AMOUNT,
        txid = %hex::encode(txid),
        "Faucet dispensed tokens"
    );

    Ok(FaucetResult {
        txid: hex::encode(txid),
        amount: FAUCET_AMOUNT.to_string(),
    })
}

/// Check and enforce the sliding-window rate limit.
///
/// Returns `true` if the request should be rejected (rate exceeded).
fn rpc_rate_limited(recent: &mut VecDeque<Instant>) -> bool {
    let now = Instant::now();
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

/// Helper to create JSON response.
fn json_response<T: Serialize>(data: T) -> Response<std::io::Cursor<Vec<u8>>> {
    let json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
    Response::from_string(json).with_header(
        "Content-Type: application/json"
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
}
