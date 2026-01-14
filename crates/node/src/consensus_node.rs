//! Networked consensus node implementation.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::{SignedProposal, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_p2p::{connect_to_peer, read_wire_message, start_listener, NetworkMessage, PeerManager};
use novai_state::MemKv;
use novai_types::Address;
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// Cache for tracking which QCs have been broadcasted (to avoid duplicates).
type QcBroadcastCache = Arc<Mutex<HashSet<(u64, u64, [u8; 32])>>>;

/// Consensus node with networking.
pub struct ConsensusNode {
    pub our_address: Address,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub state: Arc<Mutex<ConsensusState>>,
    pub db: Arc<Mutex<MemKv>>,
    pub peer_manager: Arc<PeerManager>,
    pub validator_set: Vec<Address>,
    pub validator_pubkeys: HashMap<Address, VerifyingKey>,
    pub qc_broadcasted: QcBroadcastCache,
}

impl ConsensusNode {
    pub fn new(
        signing_key: SigningKey,
        validator_set: Vec<Address>,
        validator_pubkeys: HashMap<Address, VerifyingKey>,
    ) -> Self {
        let verifying_key = signing_key.verifying_key();
        let our_address = address_from_pubkey(&verifying_key);

        Self {
            our_address,
            signing_key,
            verifying_key,
            state: Arc::new(Mutex::new(ConsensusState::new(our_address))),
            db: Arc::new(Mutex::new(MemKv::new())),
            peer_manager: Arc::new(PeerManager::new()),
            validator_set,
            validator_pubkeys,
            qc_broadcasted: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Start listening for incoming connections.
    pub fn start_listener(self: &Arc<Self>, bind_addr: SocketAddr) -> Result<(), String> {
        let node = Arc::clone(self);
        start_listener(bind_addr, self.peer_manager.clone(), move |stream| {
            let node_clone = Arc::clone(&node);
            thread::spawn(move || {
                node_clone.handle_peer_connection(stream);
            });
        })
        .map_err(|e| format!("Failed to start listener: {:?}", e))
    }

    /// Connect to a peer and start reader thread.
    pub fn connect_to_peer(self: &Arc<Self>, addr: SocketAddr) -> Result<(), String> {
        let stream =
            connect_to_peer(addr).map_err(|e| format!("Failed to connect to peer: {:?}", e))?;

        let stream_clone = stream
            .try_clone()
            .map_err(|e| format!("Failed to clone stream: {:?}", e))?;
        self.peer_manager.add_peer(stream_clone);

        let node = Arc::clone(self);
        thread::spawn(move || {
            node.handle_peer_connection(stream);
        });

        Ok(())
    }

    /// Broadcast a message to all peers.
    pub fn broadcast(&self, msg: NetworkMessage) -> Result<(), String> {
        self.peer_manager
            .broadcast(&msg)
            .map_err(|e| format!("Broadcast failed: {:?}", e))
    }

    /// Check if we're the leader for current height/round.
    /// Check if we're the leader for current height/round.
    pub fn are_we_leader(&self) -> bool {
        let state = self.state.lock().unwrap();
        match ConsensusState::compute_leader_for_view(
            state.height,
            state.round,
            &self.validator_set,
        ) {
            Ok(leader) => leader == self.our_address,
            Err(_) => false,
        }
    }

    /// Propose a block (leader only).
    pub fn propose_block(
        &self,
        mempool: &mut mempool::TxMempool,
        nonce_provider: &impl mempool::NonceProvider,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let db = self.db.lock().unwrap();

        let block = state
            .propose_block(mempool, nonce_provider, &*db, &self.validator_set)
            .map_err(|e| format!("Propose block failed: {:?}", e))?;

        let justify_qc = state.highest_qc.clone().unwrap_or(QC {
            height: 0,
            round: 0,
            block_hash: [0u8; 32],
            votes: vec![],
        });

        let proposal = novai_consensus_types::Proposal {
            block: block.clone(),
            justify_qc,
        };

        let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
            .map_err(|e| format!("Encode proposal failed: {:?}", e))?;

        let signature = sign_bytes(&self.signing_key, &unsigned_bytes);

        let signed_proposal = SignedProposal {
            proposer: self.our_address,
            proposal,
            signature,
        };

        drop(state);
        drop(db);

        println!(
            "📤 Proposing block at height={} round={}",
            block.height, block.round
        );

        self.broadcast(NetworkMessage::SignedProposal(signed_proposal))
    }

    /// Handle incoming proposal.
    /// Handle incoming proposal.
    pub fn handle_proposal(&self, signed_proposal: SignedProposal) -> Result<(), String> {
        println!(
            "📥 Received proposal from {:?}",
            &signed_proposal.proposer[..4]
        );

        let block = &signed_proposal.proposal.block;

        // 1. Check proposer is expected leader for this height/round
        // For a block at height H, the leader is determined at view_height H-1
        let view_height = block.height.saturating_sub(1);
        let expected_leader =
            ConsensusState::compute_leader_for_view(view_height, block.round, &self.validator_set)
                .map_err(|e| format!("Failed to compute leader: {:?}", e))?;

        if signed_proposal.proposer != expected_leader {
            return Err(format!(
                "Invalid proposer: expected {:?}, got {:?}",
                &expected_leader[..4],
                &signed_proposal.proposer[..4]
            ));
        }

        // 2. Verify proposal signature
        let proposer_pubkey = self
            .validator_pubkeys
            .get(&signed_proposal.proposer)
            .ok_or_else(|| {
                format!(
                    "Proposer {:?} not in validator set",
                    &signed_proposal.proposer[..4]
                )
            })?;

        let unsigned_bytes =
            novai_consensus_types::codec::encode_proposal_v1_unsigned(&signed_proposal.proposal)
                .map_err(|e| format!("Encode proposal failed: {:?}", e))?;

        if !novai_crypto::verify_bytes(proposer_pubkey, &unsigned_bytes, &signed_proposal.signature)
        {
            return Err(format!(
                "Invalid proposal signature from {:?}",
                &signed_proposal.proposer[..4]
            ));
        }

        // 3. Validate justify_qc
        let justify_qc = &signed_proposal.proposal.justify_qc;
        if block.height == 1 {
            // Height 1 MUST use GenesisQC
            if justify_qc.height != 0 || justify_qc.round != 0 || !justify_qc.votes.is_empty() {
                return Err(format!(
                    "Height 1 proposal must use GenesisQC (height=0, round=0, votes=[]), got height={} round={} votes={}",
                    justify_qc.height, justify_qc.round, justify_qc.votes.len()
                ));
            }
        } else {
            // Height > 1 MUST have valid QC with quorum
            let n = self.validator_set.len();
            let f = (n - 1) / 3;
            let quorum = 2 * f + 1;

            if justify_qc.votes.len() < quorum {
                return Err(format!(
                    "Height {} proposal has insufficient justify_qc votes: {} < quorum {}",
                    block.height,
                    justify_qc.votes.len(),
                    quorum
                ));
            }
        }

        // 4. Verify block validity
        let db = self.db.lock().unwrap();
        let state = self.state.lock().unwrap();

        state
            .verify_block(block, &*db)
            .map_err(|e| format!("Block verification failed: {:?}", e))?;

        // 5. Create vote
        let vote = state
            .create_vote(block, &self.signing_key)
            .map_err(|e| format!("Vote creation failed: {:?}", e))?;

        drop(state);
        drop(db);

        // 6. Cache block for commit rule (Week 7)
        {
            let mut state = self.state.lock().unwrap();
            state.check_no_fork(block);
            state.cache_block(block.clone());
        }

        println!("✅ Voting for block at height={}", block.height);

        self.broadcast(NetworkMessage::Vote(vote))
    }

    /// Handle incoming vote.
    pub fn handle_vote(&self, vote: Vote) -> Result<(), String> {
        println!("🗳️  Received vote from {:?}", &vote.voter[..4]);

        let mut state = self.state.lock().unwrap();

        let pubkeys_vec: Vec<(Address, VerifyingKey)> = self
            .validator_pubkeys
            .iter()
            .map(|(addr, pk)| (*addr, *pk))
            .collect();

        state
            .add_vote(vote.clone(), &pubkeys_vec)
            .map_err(|e| format!("Add vote failed: {:?}", e))?;

        // Check if we're leader for the block's height
        // Leader for height N is determined at state height N-1
        let leader_for_vote = {
            let proposal_state_height = vote.height.saturating_sub(1);
            let leader_idx =
                ((proposal_state_height + vote.round) as usize) % self.validator_set.len();
            self.validator_set[leader_idx] == self.our_address
        };

        println!(
            "DEBUG: Vote h={} r={}. Leader for proposal_height={}? {}",
            vote.height,
            vote.round,
            vote.height.saturating_sub(1),
            leader_for_vote
        );

        if !leader_for_vote {
            println!(
                "DEBUG: Not leader for height {}, skipping QC formation",
                vote.height
            );
            return Ok(());
        }

        println!(
            "DEBUG: Attempting QC formation for block_hash={:?}",
            &vote.block_hash[..8]
        );

        if let Some(qc) = state
            .try_form_qc(&vote.block_hash, &self.validator_set)
            .map_err(|e| format!("QC formation failed: {:?}", e))?
        {
            let key = (qc.height, qc.round, qc.block_hash);

            {
                let mut sent = self.qc_broadcasted.lock().unwrap();
                if sent.contains(&key) {
                    return Ok(());
                }
                sent.insert(key);
            }

            println!("🎉 QC formed with {} votes!", qc.votes.len());

            drop(state);
            self.broadcast(NetworkMessage::Qc(qc))?;
        }

        Ok(())
    }

    /// Handle incoming QC.
    pub fn handle_qc(&self, qc: QC) -> Result<(), String> {
        println!("📜 Received QC for height={} round={}", qc.height, qc.round);

        // Check commit rule and get blocks to commit (now returns Result)
        // Note: cache_qc_and_check_commit also updates highest_qc if dominated
        let to_commit = {
            let mut state = self.state.lock().unwrap();
            state
                .cache_qc_and_check_commit(qc.clone())
                .map_err(|e| format!("Commit check failed: {:?}", e))?
        };

        // Apply commits if any
        if !to_commit.is_empty() {
            let mut state = self.state.lock().unwrap();
            let mut db = self.db.lock().unwrap();

            // Calculate new committed height (last block in to_commit)
            let new_committed_height = to_commit.last().unwrap().height;

            // FIX B: Persist atomically BEFORE applying in-memory changes
            state
                .persist_commit_atomic(&mut *db, &to_commit, &qc, new_committed_height)
                .map_err(|e| format!("Atomic persist failed: {:?}", e))?;

            // Now safe to apply in-memory commits
            state.apply_commits(&to_commit);

            println!(
                "💾 Persisted state (atomic): committed_height={}, highest_qc={}",
                state.committed_height(),
                state.highest_qc.as_ref().map(|q| q.height).unwrap_or(0)
            );
        } else {
            // FIX C: Even without commit, persist highest_qc if it was updated
            let state = self.state.lock().unwrap();
            if state.highest_qc.as_ref().map(|q| q.height) == Some(qc.height) {
                // This QC became the new highest - persist it
                let mut db = self.db.lock().unwrap();
                state
                    .persist_highest_qc(&mut *db)
                    .map_err(|e| format!("Failed to persist highest QC: {:?}", e))?;
                println!(
                    "💾 Persisted highest_qc={} (no commit triggered)",
                    qc.height
                );
            }
        }

        Ok(())
    }

    /// Handle a peer connection (blocking, spawned per peer).
    pub fn handle_peer_connection(self: Arc<Self>, mut stream: TcpStream) {
        let peer_addr = stream.peer_addr().ok();
        println!("📡 Starting receive loop for peer {:?}", peer_addr);

        loop {
            match read_wire_message(&mut stream) {
                Ok(msg) => {
                    if let Err(e) = self.handle_network_message(msg) {
                        eprintln!("❌ Message handling failed: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "❌ Read failed from {:?}: {:?}, disconnecting",
                        peer_addr, e
                    );
                    break;
                }
            }
        }
    }

    /// Dispatch network message to appropriate handler.
    fn handle_network_message(&self, msg: NetworkMessage) -> Result<(), String> {
        match msg {
            NetworkMessage::SignedProposal(sp) => self.handle_proposal(sp),
            NetworkMessage::Vote(v) => self.handle_vote(v),
            NetworkMessage::Qc(qc) => self.handle_qc(qc),
        }
    }
}
