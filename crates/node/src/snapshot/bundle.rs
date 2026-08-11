//! Gate F5 Stage 2: the snapshot bundle, the artifact a healthy node produces
//! and a stranded node installs.
//!
//! Shape decision, and why it differs from the F4 operator pipeline. F4 chose a
//! full directory copy and rejected a curated key set for two reasons: a
//! history-free export cannot be certified, and every omission in a hand-built
//! export is a silent-corruption class. Both are answered here rather than
//! ignored:
//!
//! - the certification evidence travels INSIDE the bundle (`block(H)`,
//!   `block(H+1)`, the QC over `block(H+1)`, and `qc(H)` when the source has
//!   it), so A6, A7 and A8 run against a materialised bundle exactly as they
//!   run against a directory;
//! - the leaf set is derived MECHANICALLY from the same `classify` table the
//!   auditor uses, and an unclassifiable key hard-fails production instead of
//!   being dropped, which converts F4's silent-corruption class into a loud
//!   availability failure.
//!
//! A bundle is required rather than merely preferable because a directory is
//! about a gigabyte dominated by dead SMT nodes, and only the leaf set is
//! transportable. Shipping leaves rather than internal nodes is also strictly
//! stronger: the receiver rebuilds the tree itself, so a forged internal node
//! cannot survive.
//!
//! The manifest is deliberately NOT signed by the producer. A producer
//! signature would add a trust dimension the design does not need and must not
//! depend on. The only signatures that matter are the quorum votes inside the
//! carried QC.

use novai_consensus_types::codec::{decode_block_v1, decode_qc_v1, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, QC};

/// Bundle format version. A receiver refuses a manifest whose version it does
/// not know, which is the mixed-binary guard: the classification table and the
/// root identity are compiled in, so two binaries could otherwise disagree
/// about what a bundle means.
pub const SNAPSHOT_FORMAT_VERSION: u8 = 1;

/// Target payload bytes per chunk.
///
/// Sized so a chunk fits the DEFAULT 2 MiB wire send cap with four times
/// headroom, which is what lets the Stage 4 wire work enable sending without
/// also raising the cap. Nothing in Stage 2 puts a chunk on a wire; the sizing
/// is fixed now so the artifact does not have to change shape later.
pub const SNAPSHOT_CHUNK_BYTES: usize = 512 * 1024;

/// Defensive decode bounds. A malicious or corrupt input must fail fast rather
/// than drive an allocation from an attacker-chosen length.
const MAX_CHUNKS: u32 = 1_000_000;
const MAX_KEY_BYTES: u32 = 4 * 1024;
const MAX_VALUE_BYTES: u32 = 64 * 1024 * 1024;
const MAX_PAIRS_PER_CHUNK: u32 = 1_000_000;

/// A flat authenticated state set: `(key, value)` in canonical key order. The
/// leaf set a snapshot carries and the SMT is rebuilt from.
pub type FlatPairs = Vec<(Vec<u8>, Vec<u8>)>;

#[derive(Debug, PartialEq, Eq)]
pub enum BundleError {
    Truncated(&'static str),
    BadVersion(u8),
    TooLarge(&'static str, u32),
    Codec(String),
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "truncated bundle: {what}"),
            Self::BadVersion(v) => write!(f, "unknown snapshot format version {v}"),
            Self::TooLarge(what, n) => write!(f, "{what} exceeds its bound: {n}"),
            Self::Codec(e) => write!(f, "codec: {e}"),
        }
    }
}

/// What a bundle claims, and everything needed to check the claim.
///
/// Every field is redundant with something checkable. `state_root` must equal
/// the root rebuilt from the chunks AND `block_h.state_root` (the lag-0
/// identity); `chunk_digests` must match the chunk bytes; `qc_h1` must be a
/// quorum of the local validator set over `block_h1`, which must chain to
/// `block_h` by parent hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotManifest {
    pub version: u8,
    /// The snapshot height: committed == executed on the source at capture.
    pub height: u64,
    /// post-state(height). Under the post-state convention this is also
    /// `block_h.state_root`.
    pub state_root: [u8; 32],
    /// Number of authenticated leaves carried across all chunks.
    pub leaf_count: u32,
    /// blake3 over each chunk's canonical bytes, in chunk order.
    pub chunk_digests: Vec<[u8; 32]>,
    /// The block at `height`. Its header carries `state_root`.
    pub block_h: Block,
    /// The dense QC row at `height`, when the source retained one. Not
    /// required for certification; carried because a source that has it costs
    /// nothing to include and an installed directory that has it looks more
    /// like a real node directory.
    pub qc_h: Option<QC>,
    /// The block at `height + 1`, which anchors `block_h` by parent hash.
    pub block_h1: Block,
    /// The quorum certificate over `block_h1`. This is the whole trust anchor:
    /// the receiver believes the state because a quorum signed a header that
    /// commits to it, never because the sender said so.
    pub qc_h1: QC,
}

/// A manifest plus the chunk payloads it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBundle {
    pub manifest: SnapshotManifest,
    pub chunks: Vec<Vec<u8>>,
}

impl SnapshotBundle {
    /// Total payload bytes across all chunks.
    #[must_use]
    pub fn payload_bytes(&self) -> usize {
        self.chunks.iter().map(Vec::len).sum()
    }

    /// Check every chunk against the digest the manifest claims for it, and
    /// that the counts agree. This is the integrity half; it says nothing
    /// about authenticity, which only the QC and the rebuilt root can.
    ///
    /// # Errors
    /// Returns the index of the first chunk that does not match.
    pub fn verify_digests(&self) -> Result<(), String> {
        if self.chunks.len() != self.manifest.chunk_digests.len() {
            return Err(format!(
                "chunk count {} does not match the manifest's {}",
                self.chunks.len(),
                self.manifest.chunk_digests.len()
            ));
        }
        for (i, chunk) in self.chunks.iter().enumerate() {
            let got = chunk_digest(chunk);
            if got != self.manifest.chunk_digests[i] {
                return Err(format!(
                    "chunk {i} digest mismatch: computed {} manifest {}",
                    hex::encode(got),
                    hex::encode(self.manifest.chunk_digests[i])
                ));
            }
        }
        Ok(())
    }

    /// Decode every chunk back into the flat pairs, in order.
    ///
    /// # Errors
    /// Returns a decode error naming the offending chunk.
    pub fn pairs(&self) -> Result<FlatPairs, BundleError> {
        let mut out = Vec::with_capacity(self.manifest.leaf_count as usize);
        for chunk in &self.chunks {
            out.extend(decode_chunk_v1(chunk)?);
        }
        Ok(out)
    }
}

/// blake3 over a chunk's canonical bytes.
#[must_use]
pub fn chunk_digest(chunk: &[u8]) -> [u8; 32] {
    *blake3::hash(chunk).as_bytes()
}

/// Split sorted flat pairs into chunks bounded by [`SNAPSHOT_CHUNK_BYTES`].
///
/// A pair larger than the whole budget travels alone rather than being
/// dropped or splitting a value across chunks: the same floor rule the sync
/// responder uses for an oversized block. Completeness beats tidiness here,
/// because a dropped leaf is a wrong root.
#[must_use]
pub fn chunk_pairs(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<Vec<u8>> {
    let mut chunks = Vec::new();
    let mut current: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut current_bytes = 4usize; // the pair-count prefix
    for (k, v) in pairs {
        let cost = 4 + k.len() + 4 + v.len();
        if !current.is_empty() && current_bytes + cost > SNAPSHOT_CHUNK_BYTES {
            chunks.push(encode_chunk_v1(&current));
            current = Vec::new();
            current_bytes = 4;
        }
        current_bytes += cost;
        current.push((k.clone(), v.clone()));
    }
    if !current.is_empty() {
        chunks.push(encode_chunk_v1(&current));
    }
    chunks
}

/// `[u32 pair_count]` then, per pair, `[u32 key_len][key][u32 value_len][value]`.
#[must_use]
pub fn encode_chunk_v1(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(pairs.len() as u32).to_be_bytes());
    for (k, v) in pairs {
        out.extend_from_slice(&(k.len() as u32).to_be_bytes());
        out.extend_from_slice(k);
        out.extend_from_slice(&(v.len() as u32).to_be_bytes());
        out.extend_from_slice(v);
    }
    out
}

/// # Errors
/// Returns a truncation or bound error; never panics on hostile input.
pub fn decode_chunk_v1(buf: &[u8]) -> Result<FlatPairs, BundleError> {
    let mut c = Cursor::new(buf);
    let count = c.u32("chunk pair count")?;
    if count > MAX_PAIRS_PER_CHUNK {
        return Err(BundleError::TooLarge("chunk pair count", count));
    }
    let mut pairs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let k = c.bytes_u32("key", MAX_KEY_BYTES)?;
        let v = c.bytes_u32("value", MAX_VALUE_BYTES)?;
        pairs.push((k, v));
    }
    if !c.done() {
        return Err(BundleError::Truncated("trailing bytes after chunk pairs"));
    }
    Ok(pairs)
}

/// Canonical manifest encoding.
///
/// # Errors
/// Returns a codec error if a carried block or QC cannot be encoded.
pub fn encode_manifest_v1(m: &SnapshotManifest) -> Result<Vec<u8>, BundleError> {
    let mut out = Vec::new();
    out.push(m.version);
    out.extend_from_slice(&m.height.to_be_bytes());
    out.extend_from_slice(&m.state_root);
    out.extend_from_slice(&m.leaf_count.to_be_bytes());
    out.extend_from_slice(&(m.chunk_digests.len() as u32).to_be_bytes());
    for d in &m.chunk_digests {
        out.extend_from_slice(d);
    }
    put_block(&mut out, &m.block_h)?;
    match &m.qc_h {
        Some(qc) => {
            out.push(1);
            put_qc(&mut out, qc)?;
        }
        None => out.push(0),
    }
    put_block(&mut out, &m.block_h1)?;
    put_qc(&mut out, &m.qc_h1)?;
    Ok(out)
}

/// # Errors
/// Returns a truncation, bound, version or codec error.
pub fn decode_manifest_v1(buf: &[u8]) -> Result<SnapshotManifest, BundleError> {
    let mut c = Cursor::new(buf);
    let version = c.u8("version")?;
    if version != SNAPSHOT_FORMAT_VERSION {
        return Err(BundleError::BadVersion(version));
    }
    let height = c.u64("height")?;
    let state_root = c.array32("state root")?;
    let leaf_count = c.u32("leaf count")?;
    let digest_count = c.u32("chunk digest count")?;
    if digest_count > MAX_CHUNKS {
        return Err(BundleError::TooLarge("chunk digest count", digest_count));
    }
    let mut chunk_digests = Vec::with_capacity(digest_count as usize);
    for _ in 0..digest_count {
        chunk_digests.push(c.array32("chunk digest")?);
    }
    let block_h = c.block("block_h")?;
    let qc_h = match c.u8("qc_h presence")? {
        0 => None,
        1 => Some(c.qc("qc_h")?),
        _ => return Err(BundleError::Truncated("qc_h presence flag")),
    };
    let block_h1 = c.block("block_h1")?;
    let qc_h1 = c.qc("qc_h1")?;
    if !c.done() {
        return Err(BundleError::Truncated("trailing bytes after manifest"));
    }
    Ok(SnapshotManifest {
        version,
        height,
        state_root,
        leaf_count,
        chunk_digests,
        block_h,
        qc_h,
        block_h1,
        qc_h1,
    })
}

fn put_block(out: &mut Vec<u8>, b: &Block) -> Result<(), BundleError> {
    let bytes = encode_block_v1(b).map_err(|e| BundleError::Codec(format!("block: {e:?}")))?;
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&bytes);
    Ok(())
}

fn put_qc(out: &mut Vec<u8>, q: &QC) -> Result<(), BundleError> {
    let bytes = encode_qc_v1(q).map_err(|e| BundleError::Codec(format!("qc: {e:?}")))?;
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&bytes);
    Ok(())
}

struct Cursor<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, at: 0 }
    }
    fn done(&self) -> bool {
        self.at == self.buf.len()
    }
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], BundleError> {
        if self.buf.len() - self.at < n {
            return Err(BundleError::Truncated(what));
        }
        let s = &self.buf[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, BundleError> {
        Ok(self.take(1, what)?[0])
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, BundleError> {
        let s = self.take(4, what)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, BundleError> {
        let s = self.take(8, what)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_be_bytes(a))
    }
    fn array32(&mut self, what: &'static str) -> Result<[u8; 32], BundleError> {
        let s = self.take(32, what)?;
        let mut a = [0u8; 32];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn bytes_u32(&mut self, what: &'static str, max: u32) -> Result<Vec<u8>, BundleError> {
        let n = self.u32(what)?;
        if n > max {
            return Err(BundleError::TooLarge(what, n));
        }
        Ok(self.take(n as usize, what)?.to_vec())
    }
    fn block(&mut self, what: &'static str) -> Result<Block, BundleError> {
        let bytes = self.bytes_u32(what, MAX_VALUE_BYTES)?;
        let mut slice = bytes.as_slice();
        decode_block_v1(&mut slice).map_err(|e| BundleError::Codec(format!("{what}: {e:?}")))
    }
    fn qc(&mut self, what: &'static str) -> Result<QC, BundleError> {
        let bytes = self.bytes_u32(what, MAX_VALUE_BYTES)?;
        decode_qc_v1(&bytes).map_err(|e| BundleError::Codec(format!("{what}: {e:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(k: &[u8], v: &[u8]) -> (Vec<u8>, Vec<u8>) {
        (k.to_vec(), v.to_vec())
    }

    #[test]
    fn chunk_roundtrip_preserves_pairs_and_order() {
        let pairs = vec![
            pair(b"accounts/a", b"one"),
            pair(b"accounts/b", b""),
            pair(b"fee_pool", b"xyz"),
        ];
        let bytes = encode_chunk_v1(&pairs);
        assert_eq!(decode_chunk_v1(&bytes).unwrap(), pairs);
    }

    #[test]
    fn empty_value_markers_survive_the_roundtrip() {
        // Several authenticated families are presence markers with an empty
        // value (the oracle-anchor by_entity and by_tag indexes, the
        // service-descriptor by_category index). An encoding that lost them
        // would produce a wrong root, so this is a correctness case, not an
        // edge case.
        let pairs = vec![pair(b"ai/oracle_anchors/by_entity/x", b"")];
        let bytes = encode_chunk_v1(&pairs);
        assert_eq!(decode_chunk_v1(&bytes).unwrap(), pairs);
    }

    #[test]
    fn chunking_respects_the_byte_budget_and_never_drops_a_pair() {
        let big = vec![0xAB; 200 * 1024];
        let pairs: Vec<_> = (0..10u8)
            .map(|i| (vec![i; 16], big.clone()))
            .collect();
        let chunks = chunk_pairs(&pairs);
        assert!(chunks.len() > 1, "200 KiB pairs must span several chunks");
        for c in &chunks {
            assert!(
                c.len() <= SNAPSHOT_CHUNK_BYTES + 4 + 16 + 8 + big.len(),
                "no chunk may run away from the budget"
            );
        }
        let back: Vec<_> = chunks
            .iter()
            .flat_map(|c| decode_chunk_v1(c).unwrap())
            .collect();
        assert_eq!(back, pairs, "chunking must be lossless and order preserving");
    }

    #[test]
    fn an_oversized_pair_travels_alone_rather_than_being_dropped() {
        let huge = vec![7u8; SNAPSHOT_CHUNK_BYTES * 2];
        let pairs = vec![pair(b"small", b"v"), (b"huge".to_vec(), huge.clone())];
        let chunks = chunk_pairs(&pairs);
        let back: Vec<_> = chunks
            .iter()
            .flat_map(|c| decode_chunk_v1(c).unwrap())
            .collect();
        assert_eq!(back, pairs);
    }

    #[test]
    fn empty_pair_set_yields_no_chunks() {
        assert!(chunk_pairs(&[]).is_empty());
    }

    #[test]
    fn decode_rejects_truncation_rather_than_panicking() {
        let bytes = encode_chunk_v1(&[pair(b"k", b"v")]);
        for cut in 0..bytes.len() {
            assert!(
                decode_chunk_v1(&bytes[..cut]).is_err(),
                "a chunk truncated at {cut} must be rejected, never decoded"
            );
        }
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let mut bytes = encode_chunk_v1(&[pair(b"k", b"v")]);
        bytes.push(0);
        assert_eq!(
            decode_chunk_v1(&bytes),
            Err(BundleError::Truncated("trailing bytes after chunk pairs"))
        );
    }

    #[test]
    fn decode_rejects_an_attacker_chosen_length() {
        // A pair count of four billion must not drive a four billion element
        // allocation before the first byte of payload is read.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            decode_chunk_v1(&bytes),
            Err(BundleError::TooLarge("chunk pair count", _))
        ));
    }

    #[test]
    fn digest_is_stable_and_sensitive() {
        let a = encode_chunk_v1(&[pair(b"k", b"v")]);
        let b = encode_chunk_v1(&[pair(b"k", b"w")]);
        assert_eq!(chunk_digest(&a), chunk_digest(&a));
        assert_ne!(chunk_digest(&a), chunk_digest(&b));
    }
}
