use mempool::Mempool;
use novai_types::Tx;
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

fn build_tx(id: u64, payload: String) -> Tx {
    Tx {
        id,
        payload: payload.into(), // String -> Vec<u8>
    }
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

            // Minimal demo: new in-memory mempool each run
            let mut mp: Mempool<u64, Tx> = Mempool::new(|tx: &Tx| tx.id);
            let id = 1u64;

            let tx = build_tx(id, payload);
            mp.insert(tx).expect("mempool insert failed");

            println!("submitted tx id={} (mempool size={})", id, mp.len());
        }

        "drain-mempool" => {
            let payloads: Vec<String> = args.collect();
            if payloads.is_empty() {
                usage();
                return;
            }

            let mut mp: Mempool<u64, Tx> = Mempool::new(|tx: &Tx| tx.id);

            for (i, payload) in payloads.into_iter().enumerate() {
                let id = (i as u64) + 1;
                let tx = build_tx(id, payload);
                mp.insert(tx).expect("mempool insert failed");
            }

            let before = mp.len();
            let drained = mp.drain_ready(usize::MAX);
            let after = mp.len();

            let ids: Vec<u64> = drained.iter().map(|t| t.id).collect();
            println!(
                "drained {} txs (before={} after={}) ids={:?}",
                drained.len(),
                before,
                after,
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
        let mut mp: Mempool<u64, Tx> = Mempool::new(|tx: &Tx| tx.id);

        mp.insert(build_tx(1, "a".to_string())).unwrap();
        mp.insert(build_tx(2, "b".to_string())).unwrap();
        mp.insert(build_tx(3, "c".to_string())).unwrap();

        let drained = mp.drain_ready(usize::MAX);
        let ids: Vec<u64> = drained.iter().map(|t| t.id).collect();

        assert_eq!(ids, vec![1, 2, 3]);
    }
}
