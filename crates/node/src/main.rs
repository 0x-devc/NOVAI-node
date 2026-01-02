use mempool::Mempool;
use novai_codec::txid_v1;
use novai_types::{Address, SignatureBytes, TxId, TxV1, TxVersion};
use std::env;

fn usage() {
    eprintln!(
        "usage:
  novai-node submit-tx <payload>
  novai-node drain-mempool <payload> [<payload> ...]
examples:
  novai-node submit-tx hello
  novai-node drain-mempool a b c"
    );
}

fn build_tx(from: Address, nonce: u64, fee: u64, payload: String) -> TxV1 {
    // Placeholder until crypto phase: real signatures come next.
    let sig: SignatureBytes = [0u8; 64];

    TxV1 {
        version: TxVersion::V1,
        from,
        nonce,
        fee,
        payload: payload.into_bytes(),
        sig,
    }
}

fn short_hex_8(bytes32: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in bytes32.iter().take(8) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn id_of_tx(tx: &TxV1) -> TxId {
    txid_v1(tx).expect("txid computation must succeed")
}

fn main() {
    let mut args = env::args().skip(1);
    let Some(cmd) = args.next() else {
        usage();
        return;
    };

    match cmd.as_str() {
        "submit-tx" => {
            let Some(payload) = args.next() else {
                usage();
                return;
            };

            // Minimal demo: new in-memory mempool each run.
            let mut mp: Mempool<TxId, TxV1> = Mempool::new(|tx: &TxV1| id_of_tx(tx));

            // Deterministic placeholder sender until we add real keys/addresses.
            let from: Address = [0x11u8; 32];

            let tx = build_tx(from, 0, 0, payload);
            let id = id_of_tx(&tx);

            match mp.insert(tx) {
                Ok(()) => println!(
                    "submitted tx id={} (mempool size={})",
                    short_hex_8(&id),
                    mp.len()
                ),
                Err(e) => eprintln!("submit failed: {e:?}"),
            }
        }

        "drain-mempool" => {
            let payloads: Vec<String> = args.collect();
            if payloads.is_empty() {
                usage();
                return;
            }

            let mut mp: Mempool<TxId, TxV1> = Mempool::new(|tx: &TxV1| id_of_tx(tx));
            let from: Address = [0x11u8; 32];

            // Insert a batch with deterministic nonces
            for (i, p) in payloads.into_iter().enumerate() {
                let tx = build_tx(from, i as u64, 0, p);
                mp.insert(tx).expect("insert must succeed");
            }

            let before = mp.len();
            let drained = mp.drain_ready(usize::MAX);

            let ids: Vec<String> = drained
                .iter()
                .map(|tx| short_hex_8(&id_of_tx(tx)))
                .collect();

            println!(
                "drained {} txs (before={} after={}) ids={:?}",
                drained.len(),
                before,
                mp.len(),
                ids
            );
        }

        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_wires_tx_into_mempool_and_drains_in_order() {
        let mut mp: Mempool<TxId, TxV1> = Mempool::new(|tx: &TxV1| id_of_tx(tx));
        let from: Address = [0x11u8; 32];

        let tx1 = build_tx(from, 0, 0, "a".to_string());
        let tx2 = build_tx(from, 1, 0, "b".to_string());
        let tx3 = build_tx(from, 2, 0, "c".to_string());

        mp.insert(tx1).unwrap();
        mp.insert(tx2).unwrap();
        mp.insert(tx3).unwrap();

        let drained = mp.drain_ready(10);
        let payloads: Vec<Vec<u8>> = drained.into_iter().map(|t| t.payload).collect();

        assert_eq!(payloads, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }
}
