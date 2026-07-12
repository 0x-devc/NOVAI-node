//! Dev-keys validator set derivation, mirroring crates/node/src/main.rs:997-1038.
//!
//! Resolved in the F4 diagnosis (doc section 10): under --dev-keys, the live
//! validator set is exactly four ed25519 keys from the constant seeds
//! [0x00; 32] .. [0x03; 32]; the genesis file is dead input in that mode, and
//! the set is never persisted to the DB, so an offline tool must derive it
//! itself. Addresses come from novai_crypto::address_from_pubkey, the single
//! source of truth for address derivation.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_crypto::address_from_pubkey;
use novai_types::Address;

/// The four dev validator (address, pubkey) pairs, index = validator index.
pub fn dev_valset() -> Vec<(Address, VerifyingKey)> {
    (0..4u8)
        .map(|i| {
            let sk = SigningKey::from_bytes(&[i; 32]);
            let pk = sk.verifying_key();
            (address_from_pubkey(&pk), pk)
        })
        .collect()
}

/// Quorum by the 2f+1 formula with f = (n - 1) / 3, identical to the QC
/// formation site (crates/consensus/src/lib.rs:889-891) and the three
/// consensus_node verification sites. Never a hardcoded literal.
pub const fn quorum(n: usize) -> usize {
    2 * ((n - 1) / 3) + 1
}

/// Human name for a voter address: validator-N for dev validators, else
/// UNKNOWN with a short hex tag.
pub fn name_of(valset: &[(Address, VerifyingKey)], addr: &Address) -> String {
    match valset.iter().position(|(a, _)| a == addr) {
        Some(i) => format!("validator-{i}"),
        None => format!("UNKNOWN({})", hex::encode(&addr[..4])),
    }
}

/// Print the valset and quorum in the frozen CLI contract format.
pub fn print_valset() {
    let vs = dev_valset();
    for (i, (addr, pk)) in vs.iter().enumerate() {
        println!(
            "validator-{i} addr={} pubkey={}",
            hex::encode(addr),
            hex::encode(pk.as_bytes())
        );
    }
    let n = vs.len();
    let f = (n - 1) / 3;
    println!("quorum n={n} f={f} q={}", quorum(n));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quorum_formula_matches_consensus_sites() {
        assert_eq!(quorum(4), 3);
        assert_eq!(quorum(5), 3);
        assert_eq!(quorum(7), 5);
        assert_eq!(quorum(10), 7);
    }

    #[test]
    fn dev_valset_has_four_distinct_members() {
        let vs = dev_valset();
        assert_eq!(vs.len(), 4);
        for i in 0..vs.len() {
            for j in (i + 1)..vs.len() {
                assert_ne!(vs[i].0, vs[j].0, "addresses must be distinct");
            }
        }
    }

    /// Golden pin (F4 diagnosis section 10.4): the four dev validator
    /// addresses are written nowhere else in the repo, so this constant list
    /// is the regression lock for the derivation chain
    /// seed -> ed25519 verifying key -> blake3 NOVAI_ADDRESS_V1 address.
    /// The execute gate's QC voter cross-check on a healthy-node copy is
    /// read against exactly these values.
    #[test]
    fn dev_addresses_golden_pin() {
        let expected = [
            "afb67251fd9e36781878cdd0f34a86181add80b27b877e523fbcf18b958a755c",
            "33de7a15e3364aa7143b36e11ed51f0ae8003af26badc7960c095caf8b227cfb",
            "4ad1bab25752883eaa4732cc4c4a16dec0260363bf92f4be6679244045cfe698",
            "096094400c17a95bfb241040ad20788fb41673dd3ec6525303dc19700b4a075f",
        ];
        let vs = dev_valset();
        for (i, (addr, _)) in vs.iter().enumerate() {
            assert_eq!(
                hex::encode(addr),
                expected[i],
                "dev validator-{i} address drifted from the golden pin"
            );
        }
    }
}
