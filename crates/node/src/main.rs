use mempool::{NonceProvider, TxMempool};
use novai_codec::txid_v1;
use novai_crypto::{generate_keypair, sign_tx_v1};
use novai_node::consensus_node::ConsensusNode;
use novai_types::{Address, TxId, TxV1, TxVersion};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;

fn usage() {
    eprintln!(
        "usage:
  novai-node run --port <port> [--peer <addr>]... --validator <index>
  novai-node submit-tx <payload> [--nonce <u64>] [--fee <u64>] [--min-fee <u64>] [--cap <u64>]
  novai-node drain-mempool <payload> [<payload> ...] [--max <u64>] [--min-fee <u64>] [--cap <u64>]

examples:
  novai-node run --port 9000 --validator 0
  novai-node run --port 9001 --peer 127.0.0.1:9000 --validator 1
  novai-node submit-tx hello
  novai-node submit-tx hello --fee 10 --nonce 0
  novai-node drain-mempool a b c
  novai-node drain-mempool a b c --max 2
"
    );
}

fn parse_u64(opt: Option<String>, what: &str) -> u64 {
    let Some(s) = opt else {
        panic!("missing value for {what}");
    };
    s.parse::<u64>()
        .unwrap_or_else(|_| panic!("invalid {what}: {s}"))
}

#[derive(Default)]
struct InMemoryNonceProvider {
    expected: HashMap<Address, u64>,
}

impl InMemoryNonceProvider {
    fn set(&mut self, from: Address, nonce: u64) {
        self.expected.insert(from, nonce);
    }
}

impl NonceProvider for InMemoryNonceProvider {
    fn expected_nonce(&self, from: &Address) -> u64 {
        *self.expected.get(from).unwrap_or(&0)
    }
}

fn build_tx(from: Address, pubkey: [u8; 32], nonce: u64, fee: u64, payload: String) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey,
        nonce,
        fee,
        payload: payload.into_bytes(),
        sig: [0u8; 64],
    }
}

fn short_id(id: &TxId) -> String {
    // print first 8 bytes as hex for readability
    let mut s = String::new();
    for b in &id[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn main() {
    let mut args = env::args().skip(1);
    let Some(cmd) = args.next() else {
        usage();
        return;
    };

    match cmd.as_str() {
        "run" => {
            // Parse flags
            let mut port: Option<u16> = None;
            let mut peers: Vec<String> = Vec::new();
            let mut validator_idx: Option<usize> = None;

            let rest: Vec<String> = args.collect();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--port" => {
                        port = Some(parse_u64(rest.get(i + 1).cloned(), "--port") as u16);
                        i += 2;
                    }
                    "--peer" => {
                        peers.push(rest.get(i + 1).cloned().expect("missing --peer value"));
                        i += 2;
                    }
                    "--validator" => {
                        validator_idx =
                            Some(parse_u64(rest.get(i + 1).cloned(), "--validator") as usize);
                        i += 2;
                    }
                    other => {
                        panic!("unknown flag: {other}");
                    }
                }
            }

            let port = port.expect("--port required");
            let validator_idx = validator_idx.expect("--validator required");

            // Hardcoded 5-node validator set for devnet
            let validator_keys: Vec<ed25519_dalek::SigningKey> = (0..5)
                .map(|i| ed25519_dalek::SigningKey::from_bytes(&[i as u8; 32]))
                .collect();

            let validator_set: Vec<Address> = validator_keys
                .iter()
                .map(|sk| {
                    let pk = sk.verifying_key();
                    novai_crypto::address_from_pubkey(&pk)
                })
                .collect();

            let validator_pubkeys: HashMap<Address, ed25519_dalek::VerifyingKey> = validator_keys
                .iter()
                .map(|sk| {
                    let pk = sk.verifying_key();
                    (novai_crypto::address_from_pubkey(&pk), pk)
                })
                .collect();

            let our_key = validator_keys[validator_idx].clone();
            let our_addr = validator_set[validator_idx];

            println!("🚀 Starting consensus node");
            println!("   Port: {}", port);
            println!("   Validator index: {}", validator_idx);
            println!("   Address: {:?}", &our_addr[..8]);
            println!("   Peers: {:?}", peers);

            // Create node
            let node = Arc::new(ConsensusNode::new(
                our_key,
                validator_set.clone(),
                validator_pubkeys,
            ));

            // Start listener
            let bind_addr = format!("127.0.0.1:{}", port)
                .parse()
                .expect("parse bind addr");
            node.start_listener(bind_addr).expect("start listener");

            // Connect to peers (with retry)
            std::thread::sleep(Duration::from_secs(1)); // Give time for others to start
            for peer in &peers {
                let peer_addr = peer.parse().expect("parse peer addr");
                match node.connect_to_peer(peer_addr) {
                    Ok(_) => println!("✅ Connected to peer {}", peer),
                    Err(e) => println!("⚠️  Failed to connect to {}: {}", peer, e),
                }
            }

            println!("✅ Node started, waiting for peers...");
            std::thread::sleep(Duration::from_secs(2));

            // Create dummy mempool and nonce provider for Week 6
            let mut mempool = TxMempool::new(1, 1000);
            let nonce_provider = InMemoryNonceProvider::default();

            // Simple consensus loop
            loop {
                std::thread::sleep(Duration::from_secs(3));

                if node.are_we_leader() {
                    println!("👑 We are leader, proposing block...");
                    if let Err(e) = node.propose_block(&mut mempool, &nonce_provider) {
                        println!("❌ Propose failed: {}", e);
                    }
                } else {
                    println!("👂 Listening for proposals...");
                }

                // Check peer count
                let peer_count = node.peer_manager.peer_count();
                println!("   Connected peers: {}", peer_count);
            }
        }

        "submit-tx" => {
            let Some(payload) = args.next() else {
                usage();
                return;
            };

            // defaults
            let mut nonce: u64 = 0;
            let mut fee: u64 = 1;
            let mut min_fee: u64 = 1;
            let mut cap: usize = 1000;

            // parse simple flags
            let rest: Vec<String> = args.collect();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--nonce" => {
                        nonce = parse_u64(rest.get(i + 1).cloned(), "--nonce");
                        i += 2;
                    }
                    "--fee" => {
                        fee = parse_u64(rest.get(i + 1).cloned(), "--fee");
                        i += 2;
                    }
                    "--min-fee" => {
                        min_fee = parse_u64(rest.get(i + 1).cloned(), "--min-fee");
                        i += 2;
                    }
                    "--cap" => {
                        cap = parse_u64(rest.get(i + 1).cloned(), "--cap") as usize;
                        i += 2;
                    }
                    other => {
                        panic!("unknown flag: {other}");
                    }
                }
            }

            // Real Week2 mempool (policy-enforcing)
            let mut mp = TxMempool::new(min_fee, cap);

            // Dev keypair per run
            let (sk, pk) = generate_keypair();
            let from = pk.to_bytes();

            let mut nonce_provider = InMemoryNonceProvider::default();
            nonce_provider.set(from, nonce);

            let mut tx = build_tx(from, pk.to_bytes(), nonce, fee, payload);
            sign_tx_v1(&sk, &mut tx).expect("sign tx");

            let id = mp.insert(tx, &nonce_provider).expect("mempool insert");
            println!(
                "submitted tx id={} (mempool size={})",
                short_id(&id),
                mp.len()
            );
        }

        "drain-mempool" => {
            // collect payloads until flags begin
            let mut payloads: Vec<String> = Vec::new();
            let mut rest: Vec<String> = Vec::new();

            let all: Vec<String> = args.collect();
            let mut seen_flag = false;
            for a in all {
                if a.starts_with("--") {
                    seen_flag = true;
                }
                if seen_flag {
                    rest.push(a);
                } else {
                    payloads.push(a);
                }
            }

            if payloads.is_empty() {
                usage();
                return;
            }

            // defaults
            let mut max: usize = 100;
            let mut min_fee: u64 = 1;
            let mut cap: usize = 1000;

            // parse flags
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--max" => {
                        max = parse_u64(rest.get(i + 1).cloned(), "--max") as usize;
                        i += 2;
                    }
                    "--min-fee" => {
                        min_fee = parse_u64(rest.get(i + 1).cloned(), "--min-fee");
                        i += 2;
                    }
                    "--cap" => {
                        cap = parse_u64(rest.get(i + 1).cloned(), "--cap") as usize;
                        i += 2;
                    }
                    other => {
                        panic!("unknown flag: {other}");
                    }
                }
            }

            let mut mp = TxMempool::new(min_fee, cap);
            let mut nonce_provider = InMemoryNonceProvider::default();

            // Insert txs with increasing fees so drain shows fee-priority deterministically.
            let (sk, pk) = generate_keypair();
            let from = pk.to_bytes();
            nonce_provider.set(from, 0);

            for (idx, payload) in payloads.into_iter().enumerate() {
                let fee = (idx as u64) + 1;
                let mut tx = build_tx(from, pk.to_bytes(), 0, fee, payload);
                sign_tx_v1(&sk, &mut tx).expect("sign tx");

                mp.insert(tx, &nonce_provider).expect("mempool insert");
            }

            let before = mp.len();
            let drained = mp.drain_ready(max, &nonce_provider);
            let after = mp.len();

            let ids: Vec<String> = drained
                .iter()
                .map(|tx| txid_v1(tx).expect("txid").to_vec())
                .map(|id_bytes| {
                    let id: TxId = id_bytes.try_into().expect("txid size");
                    short_id(&id)
                })
                .collect();

            println!(
                "drained {} txs (before={} after={}) ids={:?}",
                drained.len(),
                before,
                after,
                ids
            );
        }

        _ => {
            usage();
        }
    }
}
