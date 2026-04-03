//! Async RPC client for communicating with a NOVAI node.

use crate::error::Error;
use crate::tx;
use novai_types::TxV1;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: serde_json::Value,
    id: u64,
}

#[derive(Deserialize)]
struct RpcResponse {
    result: Option<serde_json::Value>,
    error: Option<RpcErrorObj>,
}

#[derive(Deserialize)]
struct RpcErrorObj {
    code: i32,
    message: String,
}

/// Information about an AI entity returned from the node.
#[derive(Debug, Clone, Deserialize)]
pub struct AiEntityInfo {
    pub id: String,
    pub code_hash: String,
    pub creator: String,
    pub autonomy_mode: u8,
    pub capabilities: u8,
    pub economic_balance: String,
    pub nonce: u64,
    pub pubkey: String,
    pub memory_root: String,
    pub params_root: String,
    pub registered_at: u64,
    pub last_active_at: u64,
    pub is_active: bool,
}

/// Information about a memory object returned from the node.
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryObjectInfo {
    pub object_id: String,
    pub object_type: u8,
    pub owner_entity: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub data: String,
    pub data_size: usize,
}

/// Information about a signal commitment returned from the node.
#[derive(Debug, Clone, Deserialize)]
pub struct SignalInfo {
    pub commitment_hash: String,
    pub signal_type: u8,
    pub height: u64,
    pub issuer: String,
}

/// NOVAI RPC client.
pub struct Client {
    endpoint: String,
    http: reqwest::Client,
}

impl Client {
    /// Create a new client pointing at the given RPC endpoint.
    #[must_use]
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Send a raw JSON-RPC call.
    ///
    /// # Errors
    ///
    /// Returns error on HTTP failure or JSON-RPC error response.
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let request = RpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id,
        };

        let resp = self
            .http
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Rpc(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Rpc(format!("HTTP {status}: {body}")));
        }

        let rpc_resp: RpcResponse = resp
            .json()
            .await
            .map_err(|e| Error::Rpc(format!("failed to parse response: {e}")))?;

        if let Some(err) = rpc_resp.error {
            return Err(Error::Rpc(format!(
                "RPC error {}: {}",
                err.code, err.message
            )));
        }

        rpc_resp
            .result
            .ok_or_else(|| Error::Rpc("response has no result".to_string()))
    }

    /// Submit a signed transaction. Returns the hex-encoded txid.
    ///
    /// # Errors
    ///
    /// Returns error if encoding or RPC call fails.
    pub async fn submit_tx(&self, transaction: &TxV1) -> Result<String, Error> {
        let bytes = tx::encode_signed(transaction)?;
        let hex_str = hex::encode(&bytes);
        let result = self
            .call(
                "novai_submitTransaction",
                serde_json::json!({ "tx": hex_str }),
            )
            .await?;
        result["txid"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| Error::Rpc("missing txid in response".to_string()))
    }

    /// Query the expected nonce for an address.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_nonce(&self, address: &[u8; 32]) -> Result<u64, Error> {
        let result = self
            .call(
                "novai_getNonce",
                serde_json::json!({ "address": hex::encode(address) }),
            )
            .await?;
        result["nonce"]
            .as_u64()
            .ok_or_else(|| Error::Rpc("missing nonce in response".to_string()))
    }

    /// Query account balance and nonce.
    ///
    /// Returns `(balance, nonce)`. Balance is returned as a string because it
    /// is a u128 which exceeds JSON number precision.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_balance(&self, address: &[u8; 32]) -> Result<(String, u64), Error> {
        let result = self
            .call(
                "novai_getBalance",
                serde_json::json!({ "address": hex::encode(address) }),
            )
            .await?;
        let balance = result["balance"]
            .as_str()
            .ok_or_else(|| Error::Rpc("missing balance".to_string()))?
            .to_string();
        let nonce = result["nonce"]
            .as_u64()
            .ok_or_else(|| Error::Rpc("missing nonce".to_string()))?;
        Ok((balance, nonce))
    }

    /// Query AI entity state. Returns `None` if the entity does not exist.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_ai_entity(&self, entity_id: &[u8; 32]) -> Result<Option<AiEntityInfo>, Error> {
        let result = self
            .call(
                "novai_getAiEntity",
                serde_json::json!({ "entity_id": hex::encode(entity_id) }),
            )
            .await?;
        let entity = &result["entity"];
        if entity.is_null() {
            return Ok(None);
        }
        let info: AiEntityInfo = serde_json::from_value(entity.clone())
            .map_err(|e| Error::Rpc(format!("failed to parse entity: {e}")))?;
        Ok(Some(info))
    }

    /// Query memory objects for an entity.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_memory_objects(
        &self,
        entity_id: &[u8; 32],
    ) -> Result<Vec<MemoryObjectInfo>, Error> {
        let result = self
            .call(
                "novai_getMemoryObjects",
                serde_json::json!({ "entity_id": hex::encode(entity_id) }),
            )
            .await?;
        let objects = result["objects"]
            .as_array()
            .ok_or_else(|| Error::Rpc("missing objects".to_string()))?;
        objects
            .iter()
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|e| Error::Rpc(format!("failed to parse memory object: {e}")))
            })
            .collect()
    }

    /// Request tokens from the faucet (dev mode only).
    ///
    /// Returns `(txid_hex, amount)`.
    ///
    /// # Errors
    ///
    /// Returns error if the node is not in dev mode or the RPC call fails.
    pub async fn faucet(&self, address: &[u8; 32]) -> Result<(String, String), Error> {
        let result = self
            .call(
                "novai_faucet",
                serde_json::json!({ "address": hex::encode(address) }),
            )
            .await?;
        let txid = result["txid"]
            .as_str()
            .ok_or_else(|| Error::Rpc("missing txid".to_string()))?
            .to_string();
        let amount = result["amount"]
            .as_str()
            .ok_or_else(|| Error::Rpc("missing amount".to_string()))?
            .to_string();
        Ok((txid, amount))
    }

    /// Query signals at a specific block height.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_signals_by_height(&self, height: u64) -> Result<Vec<SignalInfo>, Error> {
        let result = self
            .call(
                "novai_getSignalsByHeight",
                serde_json::json!({ "height": height }),
            )
            .await?;
        parse_signals(&result)
    }

    /// Query signals by issuer within a height range.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_signals_by_issuer(
        &self,
        issuer: &[u8; 32],
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<SignalInfo>, Error> {
        let result = self
            .call(
                "novai_getSignalsByIssuer",
                serde_json::json!({
                    "issuer": hex::encode(issuer),
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        parse_signals(&result)
    }

    /// Query signals by type within a height range.
    ///
    /// # Errors
    ///
    /// Returns error if the RPC call fails.
    pub async fn get_signals_by_type(
        &self,
        signal_type: u8,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<SignalInfo>, Error> {
        let result = self
            .call(
                "novai_getSignalsByType",
                serde_json::json!({
                    "signal_type": signal_type,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        parse_signals(&result)
    }
}

fn parse_signals(result: &serde_json::Value) -> Result<Vec<SignalInfo>, Error> {
    let signals = result["signals"]
        .as_array()
        .ok_or_else(|| Error::Rpc("missing signals".to_string()))?;
    signals
        .iter()
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| Error::Rpc(format!("failed to parse signal: {e}")))
        })
        .collect()
}
