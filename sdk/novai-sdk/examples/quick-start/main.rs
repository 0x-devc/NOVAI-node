//! Quick-start example for `novai-sdk`.
//!
//! Walks through the four basic SDK operations:
//!   1. Connect to a node
//!   2. Generate keypairs and fund one via the dev-mode faucet
//!   3. Transfer tokens to a second account
//!   4. Register an AI entity and verify it landed on chain
//!
//! Requires a local NOVAI devnet on `http://localhost:3030`.
//! See `docs/tutorials/FIRST_AI_ENTITY.md` for devnet setup.
//!
//! Run from the repo root:
//!
//! ```bash
//! cargo run --release --example quick-start -p novai-sdk
//! ```

use std::time::Duration;

use novai_sdk::{
    keys, tx,
    types::{AutonomyMode, Capabilities},
    Client,
};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ---------------------------------------------------------------------
    // 1. Connect
    // ---------------------------------------------------------------------
    let client = Client::new("http://localhost:3030");

    let latest = client.call("novai_getLatestBlock", json!({})).await?;
    if latest.is_null() {
        return Err(
            "Chain has not committed any blocks yet. Is the devnet running? \
             See docs/tutorials/FIRST_AI_ENTITY.md for setup."
                .into(),
        );
    }
    let height = latest["height"].as_u64().unwrap_or(0);
    println!("Connected. Chain at height {height}.\n");

    // ---------------------------------------------------------------------
    // 2. Generate two keypairs (sender + recipient)
    // ---------------------------------------------------------------------
    // keys::generate() returns (SigningKey, VerifyingKey).
    // keys::address(&pk) derives the 32-byte NOVAI address from the pubkey:
    //   address = blake3("NOVAI_ADDRESS_V1" || pubkey)
    let (sender_sk, sender_pk) = keys::generate();
    let sender_addr = keys::address(&sender_pk);

    let (_recipient_sk, recipient_pk) = keys::generate();
    let recipient_addr = keys::address(&recipient_pk);

    println!("Sender    address: {}", hex::encode(sender_addr));
    println!("Recipient address: {}\n", hex::encode(recipient_addr));

    // ---------------------------------------------------------------------
    // 3. Fund the sender via the dev-mode faucet
    // ---------------------------------------------------------------------
    // Requires the node was launched with --dev-keys + --allow-insecure-dev-keys
    // (scripts/devnet.sh provides this). Dispenses 10,000,000 tokens per call,
    // 1-hour cooldown per address, 10s global cooldown.
    let (faucet_txid, amount) = client.faucet(&sender_addr).await?;
    println!(
        "Faucet dispensed {amount} tokens (tx {}…).",
        &faucet_txid[..16]
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let (initial_balance, initial_nonce) = client.get_balance(&sender_addr).await?;
    println!("Sender balance: {initial_balance}, nonce: {initial_nonce}\n");

    // ---------------------------------------------------------------------
    // 4. Send a transfer
    // ---------------------------------------------------------------------
    // tx::transfer() builds a fully signed TxV1. Amount and fee are u64.
    let transfer_amount: u64 = 100_000;
    let transfer_fee: u64 = 1_000;
    let transfer_tx = tx::transfer(
        &sender_sk,
        initial_nonce,
        transfer_fee,
        &recipient_addr,
        transfer_amount,
    )?;
    let transfer_txid = client.submit_tx(&transfer_tx).await?;
    println!(
        "Transfer {transfer_amount} tokens → recipient submitted (tx {}…).",
        &transfer_txid[..16]
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let (sender_after, sender_nonce) = client.get_balance(&sender_addr).await?;
    let (recipient_after, _) = client.get_balance(&recipient_addr).await?;
    println!("Sender    balance: {sender_after} (was {initial_balance})");
    println!("Recipient balance: {recipient_after}\n");

    // ---------------------------------------------------------------------
    // 5. Register an AI entity
    // ---------------------------------------------------------------------
    // tx::register_ai_entity_with_key() registers a new on-chain entity with
    // its own signing key (separate from the creator's). The entity gets a
    // canonical id derived from (code_hash, creator_address). The fee must
    // meet MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY (5,000).
    let (_entity_sk, entity_pk) = keys::generate();
    let code_hash = [0x01u8; 32]; // opaque placeholder
    let initial_entity_balance: u128 = 50_000;
    let register_fee: u64 = 5_000;

    let reg_tx = tx::register_ai_entity_with_key(
        &sender_sk,
        sender_nonce,
        register_fee,
        &code_hash,
        &entity_pk,
        AutonomyMode::Gated,
        Capabilities::advisory(), // read_public_chain + read_memory_objects + emit_proposals (0x07)
        initial_entity_balance,
    )?;
    let reg_txid = client.submit_tx(&reg_tx).await?;
    println!("Entity registration submitted (tx {}…).", &reg_txid[..16]);
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // ---------------------------------------------------------------------
    // 6. Verify the entity landed on chain
    // ---------------------------------------------------------------------
    // tx::compute_entity_id() mirrors the chain's deterministic derivation:
    //   entity_id = blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)
    let entity_id = tx::compute_entity_id(&code_hash, &sender_addr);
    let entity_id_hex = hex::encode(entity_id);

    let entity = client
        .get_ai_entity(&entity_id)
        .await?
        .ok_or_else(|| format!("entity {entity_id_hex} not found after register"))?;

    println!("\nEntity {}… on chain:", &entity_id_hex[..16]);
    println!("  creator:        {}", entity.creator);
    println!("  pubkey:         {}", entity.pubkey);
    println!("  balance:        {}", entity.economic_balance);
    println!("  autonomy_mode:  {} (Gated)", entity.autonomy_mode);
    println!("  capabilities:   0x{:02x}", entity.capabilities);
    println!("  registered_at:  block {}", entity.registered_at);
    println!("  is_active:      {}", entity.is_active);

    Ok(())
}
