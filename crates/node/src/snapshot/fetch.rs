//! Gate F5 Stage 5: the fetch loop.
//!
//! This is the consumer that turns an armed node into a recovering one. It
//! drives the state machine through AwaitManifest, Fetching, Verifying and
//! Staged: accept a manifest only if it proves itself, pull chunks and check
//! each against the digest the (now trusted) manifest claims, assemble the
//! bundle, and hand it to the stager.
//!
//! WHERE THE TRUST IS. Everything rests on gate 2 below: the carried QC must be
//! a quorum of THIS node's validator set over the carried successor block. Once
//! that holds, and the successor anchors `block_h`, and `block_h.state_root`
//! equals the claimed root, the manifest has proven that the root is the
//! quorum-certified post-state of its height. The serving peer is untrusted end
//! to end; nothing below gate 2 is a trust decision, only integrity.
//!
//! That ordering is deliberate: the QC is verified BEFORE a single chunk is
//! requested, so a hostile peer cannot make a node spend bandwidth on state it
//! was never going to accept.

use novai_consensus::ConsensusState;
use novai_consensus_types::codec::hash_block_v1;
use novai_consensus_types::QC;
use novai_types::Address;

use crate::snapshot::bundle::{
    chunk_digest, decode_manifest_v1, SnapshotBundle, SnapshotManifest, SNAPSHOT_FORMAT_VERSION,
};

/// How far below the local consensus frontier a snapshot may sit and still be
/// worth installing.
///
/// The design's go/no-go gate, stated in BLOCKS rather than hours so it does not
/// depend on the fleet's cadence, which is the least trustworthy number in the
/// whole system. A fifth of the retention window leaves a five times margin for
/// the catch-up that follows the install.
pub const FRESHNESS_MARGIN_BLOCKS: u64 = 10_000;

/// What this node knows about itself when it judges a manifest. Passed in
/// rather than read, so every gate is a pure decision that unit-tests without a
/// database or a network.
pub struct FetchContext<'a> {
    pub committed_height: u64,
    pub highest_qc_height: u64,
    pub voted_view: Option<(u64, u64)>,
    pub validator_pubkeys: &'a [(Address, ed25519_dalek::VerifyingKey)],
    pub quorum: usize,
}

/// Why a manifest was refused. Every variant is a refusal to spend bandwidth,
/// and none of them is recoverable by asking the same peer again.
#[derive(Debug, PartialEq, Eq)]
pub enum ManifestReject {
    Undecodable(String),
    /// Gate 1. The mixed-binary guard.
    UnknownVersion(u8),
    /// Gate 2. The trust anchor failed: no quorum signed this.
    NotCertified(String),
    /// Gate 3 or 4. The carried blocks and QC do not agree with each other.
    EvidenceInconsistent(String),
    /// Gate 5. The claimed root is not the header's, so the lag-0 identity
    /// does not hold and the manifest is describing something else.
    IdentityViolated,
    /// Gate 6. It would not move this node forward.
    NotAhead { height: u64, committed: u64 },
    /// Gate 7. Installing it could hand this node back a vote it already spent.
    WouldRegressVote { height: u64, voted: u64 },
    /// Gate 8. Too far behind the frontier to be worth installing.
    Stale { height: u64, frontier: u64 },
}

impl std::fmt::Display for ManifestReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undecodable(e) => write!(f, "manifest undecodable: {e}"),
            Self::UnknownVersion(v) => write!(f, "unknown snapshot format version {v}"),
            Self::NotCertified(e) => write!(f, "manifest is not quorum certified: {e}"),
            Self::EvidenceInconsistent(e) => write!(f, "carried evidence disagrees: {e}"),
            Self::IdentityViolated => write!(
                f,
                "header(H).state_root does not equal the claimed root; the lag-0 \
                 identity does not hold for this manifest"
            ),
            Self::NotAhead { height, committed } => write!(
                f,
                "snapshot height {height} is not ahead of committed {committed}"
            ),
            Self::WouldRegressVote { height, voted } => write!(
                f,
                "snapshot height {height} is at or below this node's highest vote {voted}"
            ),
            Self::Stale { height, frontier } => write!(
                f,
                "snapshot height {height} is more than {FRESHNESS_MARGIN_BLOCKS} blocks \
                 below the frontier {frontier}"
            ),
        }
    }
}

/// What a delivered chunk was worth.
#[derive(Debug, PartialEq, Eq)]
pub enum ChunkVerdict {
    /// Accepted and stored. `remaining` chunks still outstanding.
    Accepted { remaining: usize },
    /// Already had this one; harmless, and expected when a request is
    /// broadcast and several peers answer.
    Duplicate,
    /// Out of range for this manifest.
    UnknownIndex,
    /// For a different snapshot than the one being installed.
    WrongHeight,
    /// The bytes do not hash to what the manifest claims. The peer is broken or
    /// hostile; the caller strikes it and re-requests from someone else.
    DigestMismatch,
}

/// The fetch in progress.
#[derive(Debug, Default)]
pub struct SnapshotFetch {
    manifest: Option<SnapshotManifest>,
    chunks: Vec<Option<Vec<u8>>>,
}

impl SnapshotFetch {
    #[must_use]
    pub fn manifest(&self) -> Option<&SnapshotManifest> {
        self.manifest.as_ref()
    }

    #[must_use]
    pub fn height(&self) -> Option<u64> {
        self.manifest.as_ref().map(|m| m.height)
    }

    /// Indexes still outstanding, in order, so the caller can request them.
    #[must_use]
    pub fn missing_indexes(&self) -> Vec<u32> {
        self.chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_none())
            .filter_map(|(i, _)| u32::try_from(i).ok())
            .collect()
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.manifest.is_some() && self.chunks.iter().all(Option::is_some)
    }

    /// Abandon whatever was in flight. Used when the machine disarms.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// THE MANIFEST ACCEPTANCE GATES, in order. Gate 2 (the quorum QC) is the
    /// only trust decision; it runs before any chunk is requested, so a hostile
    /// peer cannot make this node spend bandwidth on state it would never keep.
    ///
    /// # Errors
    /// Returns the first gate that refuses, naming it.
    pub fn accept_manifest(
        &mut self,
        bytes: &[u8],
        ctx: &FetchContext<'_>,
    ) -> Result<(), ManifestReject> {
        let m = decode_manifest_v1(bytes)
            .map_err(|e| ManifestReject::Undecodable(e.to_string()))?;

        // 1. Format version. Two binaries could otherwise disagree about what a
        //    manifest means, because the classification table and the identity
        //    are compiled in.
        if m.version != SNAPSHOT_FORMAT_VERSION {
            return Err(ManifestReject::UnknownVersion(m.version));
        }

        // 2. THE TRUST ANCHOR. A quorum of THIS node's validator set, verified
        //    with the same helper the vote, proposal and sync paths use.
        ConsensusState::verify_qc_well_formed(&m.qc_h1, ctx.validator_pubkeys, ctx.quorum)
            .map_err(|e| ManifestReject::NotCertified(format!("{e:?}")))?;

        // 3. The QC certifies the block that travelled with it.
        let h1_hash = hash_block_v1(&m.block_h1)
            .map_err(|e| ManifestReject::EvidenceInconsistent(format!("hash block_h1: {e:?}")))?;
        if m.qc_h1.height != m.block_h1.height || m.qc_h1.block_hash != h1_hash {
            return Err(ManifestReject::EvidenceInconsistent(
                "the carried QC does not certify the carried successor block".to_string(),
            ));
        }

        // 4. The certified successor anchors the block whose state travels.
        let h_hash = hash_block_v1(&m.block_h)
            .map_err(|e| ManifestReject::EvidenceInconsistent(format!("hash block_h: {e:?}")))?;
        if m.block_h.height != m.height
            || m.block_h1.height != m.height + 1
            || m.block_h1.parent_hash != h_hash
        {
            return Err(ManifestReject::EvidenceInconsistent(
                "the certified successor does not anchor the snapshot's own block".to_string(),
            ));
        }

        // 5. The lag-0 identity: header(H) commits to post-state(H).
        if m.block_h.state_root != m.state_root {
            return Err(ManifestReject::IdentityViolated);
        }

        // 6. It must move this node forward.
        if m.height <= ctx.committed_height {
            return Err(ManifestReject::NotAhead {
                height: m.height,
                committed: ctx.committed_height,
            });
        }

        // 7. It must not hand this node back a vote it already spent. The
        //    install's max(own, donor) merge makes this belt and braces, and a
        //    violation means something is badly wrong, so it refuses loudly.
        if let Some((vh, _)) = ctx.voted_view {
            if m.height <= vh {
                return Err(ManifestReject::WouldRegressVote {
                    height: m.height,
                    voted: vh,
                });
            }
        }

        // 8. Freshness, in blocks. Installing a snapshot that is already near
        //    the retention wall just puts the node straight back where it was.
        if ctx.highest_qc_height.saturating_sub(m.height) > FRESHNESS_MARGIN_BLOCKS {
            return Err(ManifestReject::Stale {
                height: m.height,
                frontier: ctx.highest_qc_height,
            });
        }

        self.chunks = vec![None; m.chunk_digests.len()];
        self.manifest = Some(m);
        Ok(())
    }

    /// Take one delivered chunk. Integrity only: the manifest was already
    /// proven, so a mismatch means the PEER is wrong, not the snapshot.
    pub fn accept_chunk(&mut self, height: u64, index: u32, payload: &[u8]) -> ChunkVerdict {
        let Some(m) = &self.manifest else {
            return ChunkVerdict::WrongHeight;
        };
        if height != m.height {
            return ChunkVerdict::WrongHeight;
        }
        let Some(slot) = self.chunks.get_mut(index as usize) else {
            return ChunkVerdict::UnknownIndex;
        };
        if slot.is_some() {
            return ChunkVerdict::Duplicate;
        }
        let Some(expected) = m.chunk_digests.get(index as usize) else {
            return ChunkVerdict::UnknownIndex;
        };
        if &chunk_digest(payload) != expected {
            return ChunkVerdict::DigestMismatch;
        }
        *slot = Some(payload.to_vec());
        ChunkVerdict::Accepted {
            remaining: self.chunks.iter().filter(|c| c.is_none()).count(),
        }
    }

    /// The assembled bundle, once every chunk is in. Consumes the fetch.
    #[must_use]
    pub fn into_bundle(self) -> Option<SnapshotBundle> {
        let manifest = self.manifest?;
        let chunks: Option<Vec<Vec<u8>>> = self.chunks.into_iter().collect();
        Some(SnapshotBundle {
            manifest,
            chunks: chunks?,
        })
    }
}

/// The certifying QC a manifest carries, for a caller that wants to log it.
#[must_use]
pub fn carried_qc(m: &SnapshotManifest) -> &QC {
    &m.qc_h1
}
