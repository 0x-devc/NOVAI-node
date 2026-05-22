//! JSON-RPC 2.0 client for communicating with a NOVAI node.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Global request ID counter for JSON-RPC requests.
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: serde_json::Value,
    id: u64,
}

/// JSON-RPC 2.0 response (success or error).
#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    result: Option<serde_json::Value>,
    error: Option<RpcErrorObj>,
    #[allow(dead_code)]
    id: serde_json::Value,
}

/// JSON-RPC error object.
#[derive(Debug, Deserialize)]
struct RpcErrorObj {
    code: i32,
    message: String,
}

/// RPC client for a NOVAI node.
pub struct RpcClient {
    endpoint: String,
    http: reqwest::Client,
}

impl RpcClient {
    /// Create a new RPC client pointing at the given endpoint.
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Send an RPC call and return the result value.
    ///
    /// # Errors
    ///
    /// Returns error on HTTP failure or JSON-RPC error response.
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
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
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status}: {body}"));
        }

        let rpc_resp: RpcResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if let Some(err) = rpc_resp.error {
            return Err(format!("RPC error {}: {}", err.code, err.message));
        }

        rpc_resp
            .result
            .ok_or_else(|| "RPC response has no result".to_string())
    }

    /// Submit a hex-encoded signed transaction.
    pub async fn submit_tx(&self, tx_hex: &str) -> Result<String, String> {
        let result = self
            .call(
                "novai_submitTransaction",
                serde_json::json!({ "tx": tx_hex }),
            )
            .await?;
        let txid = result["txid"].as_str().ok_or("missing txid in response")?;
        Ok(txid.to_string())
    }

    /// Query the expected nonce for an address.
    pub async fn get_nonce(&self, address_hex: &str) -> Result<u64, String> {
        let result = self
            .call(
                "novai_getNonce",
                serde_json::json!({ "address": address_hex }),
            )
            .await?;
        result["nonce"]
            .as_u64()
            .ok_or_else(|| "missing nonce in response".to_string())
    }

    /// Query account balance and nonce.
    pub async fn get_balance(&self, address_hex: &str) -> Result<(String, u64), String> {
        let result = self
            .call(
                "novai_getBalance",
                serde_json::json!({ "address": address_hex }),
            )
            .await?;
        let balance = result["balance"]
            .as_str()
            .ok_or("missing balance in response")?
            .to_string();
        let nonce = result["nonce"]
            .as_u64()
            .ok_or("missing nonce in response")?;
        Ok((balance, nonce))
    }

    /// Query AI entity state.
    pub async fn get_ai_entity(
        &self,
        entity_id_hex: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getAiEntity",
                serde_json::json!({ "entity_id": entity_id_hex }),
            )
            .await?;
        let entity = &result["entity"];
        if entity.is_null() {
            Ok(None)
        } else {
            Ok(Some(entity.clone()))
        }
    }

    /// Query an entity's upgrade history within a height window.
    pub async fn get_upgrade_history(
        &self,
        entity_id_hex: &str,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getUpgradeHistory",
                serde_json::json!({
                    "entity_id": entity_id_hex,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        Ok(result["upgrades"].as_array().cloned().unwrap_or_default())
    }

    /// Query memory objects for an entity.
    pub async fn get_memory_objects(
        &self,
        entity_id_hex: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getMemoryObjects",
                serde_json::json!({ "entity_id": entity_id_hex }),
            )
            .await?;
        let objects = result["objects"]
            .as_array()
            .ok_or("missing objects in response")?;
        Ok(objects.clone())
    }

    /// Request tokens from the faucet.
    pub async fn faucet(&self, address_hex: &str) -> Result<(String, String), String> {
        let result = self
            .call(
                "novai_faucet",
                serde_json::json!({ "address": address_hex }),
            )
            .await?;
        let txid = result["txid"]
            .as_str()
            .ok_or("missing txid in response")?
            .to_string();
        let amount = result["amount"]
            .as_str()
            .ok_or("missing amount in response")?
            .to_string();
        Ok((txid, amount))
    }

    /// Query signals by height.
    pub async fn get_signals_by_height(
        &self,
        height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getSignalsByHeight",
                serde_json::json!({ "height": height }),
            )
            .await?;
        let signals = result["signals"]
            .as_array()
            .ok_or("missing signals in response")?;
        Ok(signals.clone())
    }

    /// Query signals by issuer.
    pub async fn get_signals_by_issuer(
        &self,
        issuer_hex: &str,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getSignalsByIssuer",
                serde_json::json!({
                    "issuer": issuer_hex,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        let signals = result["signals"]
            .as_array()
            .ok_or("missing signals in response")?;
        Ok(signals.clone())
    }

    /// Query signals by type.
    pub async fn get_signals_by_type(
        &self,
        signal_type: u8,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
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
        let signals = result["signals"]
            .as_array()
            .ok_or("missing signals in response")?;
        Ok(signals.clone())
    }

    /// Query published ServiceDescriptors by category (Week 29).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn get_service_descriptors_by_category(
        &self,
        category: u8,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getServiceDescriptorsByCategory",
                serde_json::json!({ "category": category }),
            )
            .await?;
        let descriptors = result["descriptors"]
            .as_array()
            .ok_or("missing descriptors in response")?;
        Ok(descriptors.clone())
    }

    /// Query a single VK registration by its 32-byte memory object id
    /// (Week 30).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn get_vk_registration(
        &self,
        id_hex: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getVkRegistration",
                serde_json::json!({ "id": id_hex }),
            )
            .await?;
        let registration = &result["registration"];
        if registration.is_null() {
            Ok(None)
        } else {
            Ok(Some(registration.clone()))
        }
    }

    /// Query all VK registrations owned by a given entity (Week 30).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn list_vk_registrations(
        &self,
        entity_id_hex: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_listVkRegistrations",
                serde_json::json!({ "entity_id": entity_id_hex }),
            )
            .await?;
        let registrations = result["registrations"]
            .as_array()
            .ok_or("missing registrations in response")?;
        Ok(registrations.clone())
    }

    /// Query a single SLA Agreement by its owner (buyer) entity id and
    /// 32-byte object id (Week 31).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn get_sla_agreement(
        &self,
        owner_hex: &str,
        object_id_hex: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getSlaAgreement",
                serde_json::json!({ "owner": owner_hex, "object_id": object_id_hex }),
            )
            .await?;
        let agreement = &result["agreement"];
        if agreement.is_null() {
            Ok(None)
        } else {
            Ok(Some(agreement.clone()))
        }
    }

    /// Resolve the currently-open SLA between a buyer and a seller
    /// via the active-between singleton index (Week 31).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn get_active_sla(
        &self,
        buyer_hex: &str,
        seller_hex: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getActiveSla",
                serde_json::json!({ "buyer": buyer_hex, "seller": seller_hex }),
            )
            .await?;
        let agreement = &result["agreement"];
        if agreement.is_null() {
            Ok(None)
        } else {
            Ok(Some(agreement.clone()))
        }
    }

    /// List SLA agreements where `entity_id` is the buyer (memory
    /// object owner), filtered by `created_at` in `[start, end]`
    /// (Week 31).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn list_slas_by_buyer(
        &self,
        entity_id_hex: &str,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_listSlasByBuyer",
                serde_json::json!({
                    "entity_id": entity_id_hex,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        let agreements = result["agreements"]
            .as_array()
            .ok_or("missing agreements in response")?;
        Ok(agreements.clone())
    }

    /// List SLA agreements where `entity_id` is the seller, filtered
    /// by `created_at` in `[start, end]` (Week 31).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn list_slas_by_seller(
        &self,
        entity_id_hex: &str,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_listSlasBySeller",
                serde_json::json!({
                    "entity_id": entity_id_hex,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        let agreements = result["agreements"]
            .as_array()
            .ok_or("missing agreements in response")?;
        Ok(agreements.clone())
    }

    /// Resolve a single `PaymentChannel` by its `(owner, object_id)`
    /// pair (Week 32). Returns `Ok(None)` when the channel does not
    /// exist or the resolved memory object is not a `PaymentChannel`.
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn get_payment_channel(
        &self,
        owner_hex: &str,
        object_id_hex: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_getPaymentChannel",
                serde_json::json!({ "owner": owner_hex, "object_id": object_id_hex }),
            )
            .await?;
        let channel = &result["channel"];
        if channel.is_null() {
            Ok(None)
        } else {
            Ok(Some(channel.clone()))
        }
    }

    /// List `PaymentChannel` records where `entity_id` is the
    /// memory-object owner (party A), filtered by `created_at` in
    /// `[start, end]` (Week 32).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn list_channels_by_party_a(
        &self,
        entity_id_hex: &str,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_listChannelsByPartyA",
                serde_json::json!({
                    "entity_id": entity_id_hex,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        let channels = result["channels"]
            .as_array()
            .ok_or("missing channels in response")?;
        Ok(channels.clone())
    }

    /// List `PaymentChannel` records where `entity_id` is the
    /// embedded counterparty (party B), filtered by `created_at` in
    /// `[start, end]` (Week 32).
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn list_channels_by_party_b(
        &self,
        entity_id_hex: &str,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self
            .call(
                "novai_listChannelsByPartyB",
                serde_json::json!({
                    "entity_id": entity_id_hex,
                    "start_height": start_height,
                    "end_height": end_height,
                }),
            )
            .await?;
        let channels = result["channels"]
            .as_array()
            .ok_or("missing channels in response")?;
        Ok(channels.clone())
    }

    /// Resolve dispute-window status for a `PaymentChannel` (Week 32).
    /// Returns the full response object so callers can read
    /// `found`, `status`, `closing_at_height`,
    /// `dispute_deadline_height`, `current_height`,
    /// `blocks_remaining`, and `finalize_ready`.
    ///
    /// # Errors
    ///
    /// Returns an error on HTTP failure, JSON-RPC error, or malformed
    /// response shape.
    pub async fn get_channel_dispute_status(
        &self,
        owner_hex: &str,
        object_id_hex: &str,
    ) -> Result<serde_json::Value, String> {
        let result = self
            .call(
                "novai_getChannelDisputeStatus",
                serde_json::json!({ "owner": owner_hex, "object_id": object_id_hex }),
            )
            .await?;
        Ok(result)
    }
}
