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

use mempool::{NonceProvider, TxMempool};
use novai_ai_entities::{AiSignalType, SignalCommitment};
use novai_codec::{decode_tx_v1_signed, txid_v1};
use novai_execution::{get_signals_by_height, get_signals_by_issuer, get_signals_by_type};
use novai_state::MemKv;
use novai_types::{Address, TxV1};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use tiny_http::{Response, Server, StatusCode};

/// No-op nonce provider that accepts all nonces.
///
/// TODO: Replace with actual state-backed nonce provider that queries
/// current account nonces from the execution engine.
struct NoOpNonceProvider;

impl NonceProvider for NoOpNonceProvider {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0 // Accept all nonces for now
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

/// Start the JSON-RPC server.
///
/// Spawns a dedicated thread to handle HTTP JSON-RPC requests.
/// Returns immediately after starting the listener.
///
/// # Arguments
/// - `bind_addr` - Address to bind the HTTP server (e.g., "0.0.0.0:9545")
/// - `mempool` - Shared mempool for transaction submission
///
/// # Errors
/// Returns error if the server cannot bind to the address (e.g., port in use).
pub fn start_rpc_server(bind_addr: &str, mempool: Arc<Mutex<TxMempool>>) -> Result<(), String> {
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start RPC server: {e}"))?;

    println!("🔌 RPC server listening on http://{}", addr);

    thread::spawn(move || {
        for mut request in server.incoming_requests() {
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
                "novai_submitTransaction" => match handle_submit_tx(&rpc_request, &mempool) {
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
/// Extended server that supports both transaction submission and signal queries.
///
/// # Arguments
/// - `bind_addr` - Address to bind the HTTP server (e.g., "0.0.0.0:9545")
/// - `mempool` - Shared mempool for transaction submission
/// - `db` - Shared state database for signal queries
///
/// # Errors
/// Returns error if the server cannot bind to the address.
pub fn start_rpc_server_with_state(
    bind_addr: &str,
    mempool: Arc<Mutex<TxMempool>>,
    db: Arc<Mutex<MemKv>>,
) -> Result<(), String> {
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start RPC server: {e}"))?;

    println!(
        "🔌 RPC server listening on http://{} (with state queries)",
        addr
    );

    thread::spawn(move || {
        for mut request in server.incoming_requests() {
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
                "novai_submitTransaction" => match handle_submit_tx(&rpc_request, &mempool) {
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
    let mut mempool = mempool.lock().unwrap();
    let nonce_provider = NoOpNonceProvider;
    mempool.insert(tx, &nonce_provider).map_err(|e| RpcError {
        code: -32001,
        message: format!("Mempool rejected transaction: {:?}", e),
    })?;

    // Return success response
    Ok(SubmitTxResult {
        txid: hex::encode(txid),
    })
}

// ============================================================================
// SIGNAL QUERY HANDLERS (Week 14 - D14.5)
// ============================================================================

/// Handle novai_getSignalsByHeight RPC method.
fn handle_get_signals_by_height(
    request: &RpcRequest,
    db: &Arc<Mutex<MemKv>>,
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
    db: &Arc<Mutex<MemKv>>,
) -> Result<SignalQueryResult, RpcError> {
    let params: GetSignalsByIssuerParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {}", e),
        })?;

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
    db: &Arc<Mutex<MemKv>>,
) -> Result<SignalQueryResult, RpcError> {
    let params: GetSignalsByTypeParams =
        serde_json::from_value(request.params.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid params: {}", e),
        })?;

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
