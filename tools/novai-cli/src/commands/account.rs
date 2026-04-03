//! Account commands: balance, nonce, faucet, transfer.

use crate::commands::keygen::{load_keypair, parse_hex32};
use crate::rpc_client::RpcClient;
use novai_codec::{encode_tx_v1_signed, txid_v1};
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_types::{TxV1, TxVersion};

/// Build, sign, and submit a transaction. Returns the txid hex.
pub async fn sign_and_submit(
    rpc: &RpcClient,
    key_file: &str,
    payload: Vec<u8>,
    fee: u64,
) -> Result<String, String> {
    let (sk, pk) = load_keypair(key_file)?;
    let addr = address_from_pubkey(&pk);
    let addr_hex = hex::encode(addr);

    let nonce = rpc.get_nonce(&addr_hex).await?;

    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: addr,
        pubkey: pk.to_bytes(),
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    };

    sign_tx_v1(&sk, &mut tx).map_err(|e| format!("Failed to sign: {e:?}"))?;

    let txid = txid_v1(&tx).map_err(|e| format!("Failed to compute txid: {e:?}"))?;
    let tx_bytes = encode_tx_v1_signed(&tx).map_err(|e| format!("Failed to encode tx: {e:?}"))?;
    let tx_hex = hex::encode(&tx_bytes);

    rpc.submit_tx(&tx_hex).await?;

    Ok(hex::encode(txid))
}

/// Query account balance.
pub async fn run_balance(rpc: &RpcClient, address: &str, json: bool) -> Result<(), String> {
    let (balance, nonce) = rpc.get_balance(address).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "balance": balance, "nonce": nonce })
        );
    } else {
        println!("Balance: {balance}");
        println!("Nonce:   {nonce}");
    }
    Ok(())
}

/// Query account nonce.
pub async fn run_nonce(rpc: &RpcClient, address: &str, json: bool) -> Result<(), String> {
    let nonce = rpc.get_nonce(address).await?;
    if json {
        println!("{}", serde_json::json!({ "nonce": nonce }));
    } else {
        println!("Nonce: {nonce}");
    }
    Ok(())
}

/// Request testnet tokens from the faucet.
pub async fn run_faucet(rpc: &RpcClient, address: &str, json: bool) -> Result<(), String> {
    let (txid, amount) = rpc.faucet(address).await?;
    if json {
        println!("{}", serde_json::json!({ "txid": txid, "amount": amount }));
    } else {
        println!("Faucet dispensed {amount} tokens");
        println!("TxID: {txid}");
    }
    Ok(())
}

/// Transfer tokens to another address.
pub async fn run_transfer(
    rpc: &RpcClient,
    key_file: &str,
    to: &str,
    amount: u64,
    fee: u64,
    json: bool,
) -> Result<(), String> {
    let to_addr = parse_hex32(to, "to")?;

    // Build transfer payload: [version:1][to:32][amount:8 BE]
    let mut payload = Vec::with_capacity(41);
    payload.push(1); // Transfer payload version
    payload.extend_from_slice(&to_addr);
    payload.extend_from_slice(&amount.to_be_bytes());

    let txid = sign_and_submit(rpc, key_file, payload, fee).await?;

    if json {
        println!("{}", serde_json::json!({ "txid": txid, "amount": amount }));
    } else {
        println!("Transfer submitted");
        println!("Amount: {amount}");
        println!("To:     {to}");
        println!("TxID:   {txid}");
    }
    Ok(())
}
