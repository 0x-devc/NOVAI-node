//! JSON-RPC 2.0 server for transaction submission.
//!
//! PURPOSE: Provide HTTP JSON-RPC endpoint for external clients to submit transactions.
//!
//! INVARIANTS:
//! - Server binds to specified address on startup
//! - Accepts only JSON-RPC 2.0 requests
//! - Returns txid on successful submission to mempool
//!
//! FAILURE MODES:
//! - Port already in use → returns error on start
//! - Invalid transaction → returns RPC error -32000
//! - Mempool full → returns RPC error -32001

use mempool::{NonceProvider, TxMempool};
use novai_codec::{decode_tx_v1_signed, txid_v1};
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
