//! Option A pre-fund helper for load-test sender scaling.
//!
//! PURPOSE
//! The dev-genesis path funds only sender indices 0..99
//! (`apply_dev_genesis`, crates/node/src/main.rs:602-627). To run the load
//! generator with ~150 funded senders (the count needed for ~50 valid
//! tx/block), the extra indices 100..149 must be funded once from an
//! already-funded account. This helper derives those target addresses and
//! builds one signed Transfer tx per target.
//!
//! REUSE, NOT REIMPLEMENT
//! Signing, encoding, txid, and address derivation all go through the same
//! workspace crates the node and generator use (novai-crypto, novai-codec,
//! novai-types), so the produced bytes are identical to what
//! crates/tx-generator/src/submitter.rs:248-272 produces. The seed
//! derivation mirrors crates/tx-generator/src/sender.rs:38-60 and the
//! node's own replica at crates/node/src/main.rs:607-627. The payload
//! wire format mirrors crates/tx-generator/src/generator.rs:268-272 and is
//! the format the node decodes at crates/execution/src/lib.rs:1144-1166.
//!
//! SAFETY
//! `--dry-run` performs NO network I/O: it builds and signs locally and
//! prints the plan plus one sample signed tx. Real submission requires the
//! absence of `--dry-run` AND an explicit `--confirm` flag, and even then
//! the helper checks the source balance and paces submission under the
//! node's per-sender mempool cap before sending anything.

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use ed25519_dalek::SigningKey;
use novai_codec::{encode_tx_v1_signed, txid_v1};
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_types::{Address, TxV1, TxVersion};
use std::time::Duration;

/// Per-sender pending-tx cap enforced by the node mempool
/// (`mempool::MAX_PENDING_PER_SENDER`, crates/mempool/src/lib.rs:9). The
/// in-flight window MUST stay strictly below this, or the node rejects with
/// `SenderLimitExceeded` (-32012, crates/node/src/rpc.rs:2051-2053).
const NODE_MAX_PENDING_PER_SENDER: u64 = 16;

/// Minimum balance a brand-new recipient account must receive
/// (`MIN_ACCOUNT_BALANCE`, crates/execution/src/lib.rs:12124). New-account
/// transfers below this are rejected at execution (lib.rs:6665-6669).
const MIN_ACCOUNT_BALANCE: u64 = 1_000;

/// Balance dev-genesis assigns each funded account
/// (crates/node/src/main.rs:603). Used only to sanity-print the source's
/// expected headroom in dry-run.
const DEV_GENESIS_BALANCE: u128 = 1_000_000_000;

#[derive(Parser, Debug)]
#[command(
    name = "prefund-senders",
    about = "Option A: pre-fund tx-generator sender accounts (no node source change)"
)]
struct Args {
    /// First sender index to fund (inclusive).
    #[arg(long, default_value_t = 100)]
    start_index: usize,

    /// Number of consecutive sender indices to fund.
    #[arg(long, default_value_t = 50)]
    count: usize,

    /// Already-funded source account index. Dev-genesis funds 0..99.
    #[arg(long, default_value_t = 0)]
    source_index: usize,

    /// Amount credited to each target. Must be >= MIN_ACCOUNT_BALANCE (1000).
    #[arg(long, default_value_t = 10_000_000)]
    amount: u64,

    /// Fee per funding tx. Node min_fee is 1 (crates/node/src/main.rs:1190);
    /// 1000 matches the generator default (crates/tx-generator/src/main.rs:44-45).
    #[arg(long, default_value_t = 1000)]
    fee: u64,

    /// RPC endpoint. Validator RPC ports are 3030..3033, NOT 8080 (metrics).
    #[arg(long, default_value = "http://localhost:3030")]
    endpoint: String,

    /// Starting source nonce for --dry-run display and real-mode fallback.
    /// Real mode overrides this by querying novai_getBalance for the source.
    #[arg(long, default_value_t = 0)]
    source_nonce: u64,

    /// Max funding txs in flight from the source at once. Must be < 16.
    #[arg(long, default_value_t = 1)]
    in_flight: u64,

    /// Poll interval (ms) while waiting for the source nonce to advance (real mode).
    #[arg(long, default_value_t = 500)]
    poll_interval_ms: u64,

    /// Max polls to wait for a single nonce to commit before giving up (real mode).
    #[arg(long, default_value_t = 240)]
    max_polls: u64,

    /// Print the plan and a sample signed tx. Performs NO network I/O.
    #[arg(long)]
    dry_run: bool,

    /// Required to actually submit. Without it, real mode refuses to send.
    #[arg(long)]
    confirm: bool,
}

/// Replicate `SenderAccount::from_index` (crates/tx-generator/src/sender.rs:38-60).
/// Returns (signing key, pubkey bytes, address).
fn derive_account(index: usize) -> (SigningKey, [u8; 32], Address) {
    let seed_byte = (index % 256) as u8;
    let mut seed = [seed_byte; 32];
    let index_bytes = index.to_le_bytes();
    for (i, &b) in index_bytes.iter().enumerate() {
        seed[i] ^= b;
    }
    let sk = SigningKey::from_bytes(&seed);
    let vk = sk.verifying_key();
    let addr = address_from_pubkey(&vk);
    (sk, vk.to_bytes(), addr)
}

/// Build the TransferPayloadV1 wire bytes `[version:1][to:32][amount:8 BE]`
/// (crates/tx-generator/src/generator.rs:268-272; decoded by the node at
/// crates/execution/src/lib.rs:1144-1166).
fn transfer_payload(to: &Address, amount: u64) -> Vec<u8> {
    let mut p = Vec::with_capacity(1 + 32 + 8);
    p.push(1);
    p.extend_from_slice(to);
    p.extend_from_slice(&amount.to_be_bytes());
    p
}

/// Build and sign one Transfer tx, mirroring submitter.rs:248-272. Returns
/// (signed wire bytes, txid).
fn build_signed_transfer(
    source_sk: &SigningKey,
    source_pubkey: [u8; 32],
    source_addr: Address,
    to: &Address,
    amount: u64,
    fee: u64,
    nonce: u64,
) -> Result<(Vec<u8>, [u8; 32])> {
    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: source_addr,
        pubkey: source_pubkey,
        nonce,
        fee,
        payload: transfer_payload(to, amount),
        sig: [0u8; 64],
    };
    sign_tx_v1(source_sk, &mut tx).map_err(|e| anyhow!("sign_tx_v1 failed: {e:?}"))?;
    let bytes = encode_tx_v1_signed(&tx).map_err(|e| anyhow!("encode_tx_v1_signed failed: {e:?}"))?;
    let txid = txid_v1(&tx).map_err(|e| anyhow!("txid_v1 failed: {e:?}"))?;
    Ok((bytes, txid))
}

/// One planned funding transfer.
struct Plan {
    seq: usize,
    target_index: usize,
    target_addr: Address,
    nonce: u64,
    bytes: Vec<u8>,
    txid: [u8; 32],
}

fn build_plan(args: &Args, base_nonce: u64) -> Result<(Address, Vec<Plan>)> {
    if args.count == 0 {
        bail!("--count must be > 0");
    }
    if args.amount < MIN_ACCOUNT_BALANCE {
        bail!(
            "--amount {} is below MIN_ACCOUNT_BALANCE {} (new recipients would be rejected at \
             crates/execution/src/lib.rs:6665)",
            args.amount,
            MIN_ACCOUNT_BALANCE
        );
    }
    if args.in_flight == 0 || args.in_flight >= NODE_MAX_PENDING_PER_SENDER {
        bail!(
            "--in-flight {} must be in 1..{} (node per-sender cap, crates/mempool/src/lib.rs:9)",
            args.in_flight,
            NODE_MAX_PENDING_PER_SENDER
        );
    }
    // Guard against the source index overlapping the funded range: a source
    // inside [start, start+count) would be funding itself and skew nonces.
    if args.source_index >= args.start_index && args.source_index < args.start_index + args.count {
        bail!(
            "--source-index {} overlaps the target range {}..{}; choose a funded source outside it",
            args.source_index,
            args.start_index,
            args.start_index + args.count
        );
    }

    let (source_sk, source_pubkey, source_addr) = derive_account(args.source_index);

    let mut plans = Vec::with_capacity(args.count);
    for seq in 0..args.count {
        let target_index = args.start_index + seq;
        let (_sk, _pk, target_addr) = derive_account(target_index);
        let nonce = base_nonce + seq as u64;
        let (bytes, txid) = build_signed_transfer(
            &source_sk,
            source_pubkey,
            source_addr,
            &target_addr,
            args.amount,
            args.fee,
            nonce,
        )?;
        plans.push(Plan {
            seq,
            target_index,
            target_addr,
            nonce,
            bytes,
            txid,
        });
    }
    Ok((source_addr, plans))
}

fn print_dry_run(args: &Args, source_addr: &Address, base_nonce: u64, plans: &[Plan]) {
    let total_amount = (plans.len() as u128) * u128::from(args.amount);
    let total_fee = (plans.len() as u128) * u128::from(args.fee);
    let total_debit = total_amount + total_fee;

    println!("=== prefund-senders DRY RUN (no network I/O) ===");
    println!(
        "source: index={} address={}",
        args.source_index,
        hex::encode(source_addr)
    );
    println!(
        "assumed source base nonce (from --source-nonce): {} \
         (real mode queries novai_getBalance instead)",
        base_nonce
    );
    println!(
        "targets: indices {}..{} ({} accounts)",
        args.start_index,
        args.start_index + args.count,
        args.count
    );
    println!(
        "per target: amount={} fee={} (amount >= MIN_ACCOUNT_BALANCE {}: OK)",
        args.amount, args.fee, MIN_ACCOUNT_BALANCE
    );
    println!(
        "totals: amount={} fee={} debit_from_source={} (dev-genesis funds each account {})",
        total_amount, total_fee, total_debit, DEV_GENESIS_BALANCE
    );
    if total_debit > DEV_GENESIS_BALANCE {
        println!(
            "WARNING: total debit {} exceeds a single dev-genesis balance {}; \
             lower --amount or use a source with more funds",
            total_debit, DEV_GENESIS_BALANCE
        );
    }
    println!(
        "pacing: in_flight={} (node per-sender cap is {}); funding txs drain ~1 per commit \
         window for the source, so real mode polls the source nonce between sends",
        args.in_flight, NODE_MAX_PENDING_PER_SENDER
    );
    println!(
        "note: the source ({}) is also a load sender; its first generator txs will draw a few \
         NonceTooLow (-32010) until auto-resync (crates/tx-generator/src/submitter.rs:145,287-316)",
        args.source_index
    );
    println!();
    println!("planned funding txs (seq | target_index | nonce | amount | target_address):");
    for p in plans {
        println!(
            "  {:>3} | idx {:>4} | nonce {:>4} | amt {:>12} | {}",
            p.seq,
            p.target_index,
            p.nonce,
            args.amount,
            hex::encode(p.target_addr)
        );
    }
    println!();
    println!("built and signed {} transactions locally (no network).", plans.len());
    if let Some(first) = plans.first() {
        println!(
            "sample tx[0]: nonce={} txid={} bytes_len={}",
            first.nonce,
            hex::encode(first.txid),
            first.bytes.len()
        );
        println!("sample tx[0] signed wire hex:");
        println!("  {}", hex::encode(&first.bytes));
    }
    println!();
    println!("DRY RUN complete. Nothing was sent. To submit for real (NOT this session):");
    println!(
        "  prefund-senders --endpoint <http://HOST:303x> --source-index {} \
         --start-index {} --count {} --amount {} --fee {} --confirm",
        args.source_index, args.start_index, args.count, args.amount, args.fee
    );
}

async fn rpc_call(
    client: &reqwest::Client,
    endpoint: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let body = serde_json::json!({"jsonrpc":"2.0","method":method,"params":params,"id":1});
    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("RPC request to {endpoint} ({method}) failed"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .with_context(|| format!("RPC response from {method} was not JSON (http {status})"))?;
    if let Some(err) = v.get("error") {
        if !err.is_null() {
            bail!("RPC error from {method}: {err}");
        }
    }
    Ok(v)
}

async fn query_balance_nonce(
    client: &reqwest::Client,
    endpoint: &str,
    addr: &Address,
) -> Result<(u128, u64)> {
    let v = rpc_call(
        client,
        endpoint,
        "novai_getBalance",
        serde_json::json!({ "address": hex::encode(addr) }),
    )
    .await?;
    let result = v.get("result").ok_or_else(|| anyhow!("missing result"))?;
    let balance: u128 = result
        .get("balance")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("missing result.balance"))?
        .parse()
        .context("balance not a u128 decimal string")?;
    let nonce = result
        .get("nonce")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("missing result.nonce"))?;
    Ok((balance, nonce))
}

async fn submit_tx(client: &reqwest::Client, endpoint: &str, bytes: &[u8]) -> Result<String> {
    let v = rpc_call(
        client,
        endpoint,
        "novai_submitTransaction",
        serde_json::json!({ "tx": hex::encode(bytes) }),
    )
    .await?;
    let txid = v
        .get("result")
        .and_then(|r| r.get("txid"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("missing result.txid"))?;
    Ok(txid.to_string())
}

/// Real submission path. NOT exercised this session (requires --confirm and a
/// reachable endpoint). Paces sends so at most `in_flight` funding txs are
/// pending from the source at once, polling the source nonce to confirm
/// commits and never exceeding the node per-sender cap.
async fn run_real(args: &Args) -> Result<()> {
    if !args.confirm {
        bail!(
            "real submission requires --confirm. Re-run with --dry-run to preview, or add \
             --confirm to send {} funding txs to {}",
            args.count,
            args.endpoint
        );
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("failed to build HTTP client")?;

    let (_sk, _pk, source_addr) = derive_account(args.source_index);

    // Source nonce and balance come from the live chain, not the flag.
    let (balance, base_nonce) = query_balance_nonce(&client, &args.endpoint, &source_addr).await?;
    println!(
        "source index={} address={} chain_balance={} chain_nonce={}",
        args.source_index,
        hex::encode(source_addr),
        balance,
        base_nonce
    );

    let (_source_addr, plans) = build_plan(args, base_nonce)?;

    let total_debit = (plans.len() as u128) * (u128::from(args.amount) + u128::from(args.fee));
    if balance < total_debit {
        bail!(
            "source balance {} < required {} (count {} x (amount {} + fee {}))",
            balance,
            total_debit,
            plans.len(),
            args.amount,
            args.fee
        );
    }

    let end_nonce = base_nonce + plans.len() as u64;
    let mut next = 0usize; // index into plans of the next tx to submit
    let mut confirmed_nonce = base_nonce; // all source nonces < this are committed

    while confirmed_nonce < end_nonce {
        // Fill the in-flight window.
        while next < plans.len() && (plans[next].nonce - confirmed_nonce) < args.in_flight {
            let p = &plans[next];
            let txid = submit_tx(&client, &args.endpoint, &p.bytes).await?;
            println!(
                "submitted seq={} idx={} nonce={} txid={}",
                p.seq, p.target_index, p.nonce, txid
            );
            next += 1;
        }

        // Wait for the source nonce to advance (a funding tx committed).
        let mut polls = 0u64;
        loop {
            tokio::time::sleep(Duration::from_millis(args.poll_interval_ms)).await;
            let (_bal, nonce) = query_balance_nonce(&client, &args.endpoint, &source_addr).await?;
            if nonce > confirmed_nonce {
                confirmed_nonce = nonce;
                println!("progress: source nonce advanced to {confirmed_nonce}/{end_nonce}");
                break;
            }
            polls += 1;
            if polls >= args.max_polls {
                bail!(
                    "source nonce stuck at {} after {} polls; chain may be stalled or a funding \
                     tx was rejected (check node logs)",
                    confirmed_nonce,
                    polls
                );
            }
        }
    }

    println!(
        "funding complete: source nonce {} -> {} ({} accounts funded)",
        base_nonce,
        confirmed_nonce,
        plans.len()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.dry_run {
        let base_nonce = args.source_nonce;
        let (source_addr, plans) = build_plan(&args, base_nonce)?;
        print_dry_run(&args, &source_addr, base_nonce, &plans);
        return Ok(());
    }

    run_real(&args).await
}
