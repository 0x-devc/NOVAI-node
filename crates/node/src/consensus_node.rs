//! Networked consensus node implementation.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::{SignedProposal, Timeout, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_p2p::{connect_to_peer, read_wire_message, start_listener, NetworkMessage, PeerManager};
use novai_state::{Kv, MemKv};
use novai_types::Address;
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

/// Cache for tracking which QCs have been broadcasted (to avoid duplicates).
type QcBroadcastCache = Arc<Mutex<HashSet<(u64, u64, [u8; 32])>>>;

/// Consensus node with networking.
/// Tracks a pending block sync request.
#[derive(Debug, Clone)]
pub struct PendingSyncRequest {
    pub peer: Address,
    pub start_height: u64,
    pub end_height: u64,
    pub request_time: Instant,
}

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
    pub round_start_time: Arc<Mutex<Instant>>,
    pub timed_out_this_round: Arc<Mutex<bool>>,
    pub pending_sync_request: Arc<Mutex<Option<PendingSyncRequest>>>,
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
            round_start_time: Arc::new(Mutex::new(Instant::now())),
            timed_out_this_round: Arc::new(Mutex::new(false)),
            pending_sync_request: Arc::new(Mutex::new(None)),
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

    /// Check if timeout should be triggered and create it.
    ///
    /// Returns Some(Timeout) if timeout duration elapsed and not already timed out.
    pub fn check_timeout(&self) -> Option<Timeout> {
        let timed_out = *self.timed_out_this_round.lock().unwrap();
        if timed_out {
            return None; // Already timed out this round
        }

        let start_time = *self.round_start_time.lock().unwrap();
        let state = self.state.lock().unwrap();

        let timeout_duration =
            std::time::Duration::from_millis(novai_consensus::timeout_for_round(state.round));

        if start_time.elapsed() >= timeout_duration {
            // Create timeout
            match state.create_timeout(&self.signing_key) {
                Ok(timeout) => {
                    *self.timed_out_this_round.lock().unwrap() = true;
                    println!(
                        "⏰ TIMEOUT triggered for round={} after {:?}",
                        state.round,
                        start_time.elapsed()
                    );
                    Some(timeout)
                }
                Err(e) => {
                    eprintln!("❌ Failed to create timeout: {:?}", e);
                    None
                }
            }
        } else {
            None
        }
    }

    /// Handle incoming timeout message.
    ///
    /// Returns Ok(true) if round was advanced, Ok(false) otherwise.
    pub fn handle_timeout(&self, timeout: Timeout) -> Result<bool, String> {
        println!("⏰ Received timeout from {:?}", &timeout.voter[..4]);

        let pubkeys_vec: Vec<(Address, VerifyingKey)> = self
            .validator_pubkeys
            .iter()
            .map(|(addr, pk)| (*addr, *pk))
            .collect();

        let mut state = self.state.lock().unwrap();

        // Add timeout to state
        state
            .add_timeout(timeout, &pubkeys_vec)
            .map_err(|e| format!("Add timeout failed: {:?}", e))?;

        // Try to advance round
        let advanced = state.try_advance_round(&self.validator_set);

        if advanced {
            // Reset round timer
            *self.round_start_time.lock().unwrap() = Instant::now();
            *self.timed_out_this_round.lock().unwrap() = false;

            println!(
                "⏰ ROUND ADVANCED to round={} at height={}",
                state.round,
                state.height + 1
            );
        }

        Ok(advanced)
    }

    /// Request blocks from a peer for catch-up.
    ///
    /// Returns `Ok(())` if request was sent, or error if already pending or no peers available.
    pub fn request_blocks_from_peer(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<(), String> {
        // Check if there's already a pending request
        let mut pending = self.pending_sync_request.lock().unwrap();
        if pending.is_some() {
            return Err("Sync request already pending".to_string());
        }

        // Select a peer (simple: just take the first validator that's not us)
        let peer = self
            .validator_set
            .iter()
            .find(|&&addr| addr != self.our_address)
            .copied()
            .ok_or_else(|| "No peers available for sync".to_string())?;

        // Create and send request
        let request = novai_consensus_types::BlockRequest {
            requester: self.our_address,
            start_height,
            end_height,
        };

        println!(
            "📥 Requesting blocks {}-{} from peer {:?}",
            start_height,
            end_height,
            &peer[..4]
        );

        // Store pending request
        *pending = Some(PendingSyncRequest {
            peer,
            start_height,
            end_height,
            request_time: Instant::now(),
        });

        drop(pending);

        // Broadcast request
        self.broadcast(NetworkMessage::BlockRequest(request))?;

        Ok(())
    }

    /// Handle incoming block request from a peer.
    pub fn handle_block_request(
        &self,
        request: novai_consensus_types::BlockRequest,
    ) -> Result<(), String> {
        println!(
            "📤 Received block request for {}-{} from {:?}",
            request.start_height,
            request.end_height,
            &request.requester[..4]
        );

        let db = self.db.lock().unwrap();

        // Load individual blocks, stop at first missing
        let mut blocks = Vec::new();
        for height in request.start_height..=request.end_height {
            match ConsensusState::load_block(&*db, height) {
                Ok(Some(block)) => blocks.push(block),
                _ => break, // Stop at first missing block
            }
        }

        drop(db);

        println!(
            "📤 Sending {} blocks (requested {}-{}) to {:?}",
            blocks.len(),
            request.start_height,
            request.end_height,
            &request.requester[..4]
        );

        // Send response with whatever blocks we have
        let response = novai_consensus_types::BlockResponse {
            responder: self.our_address,
            request_start: request.start_height,
            request_end: request.end_height,
            blocks,
        };

        self.broadcast(NetworkMessage::BlockResponse(response))?;

        Ok(())
    }

    /// Handle incoming block response from a peer.
    pub fn handle_block_response(
        &self,
        response: novai_consensus_types::BlockResponse,
    ) -> Result<(), String> {
        println!(
            "📥 Received {} blocks from {:?}",
            response.blocks.len(),
            &response.responder[..4]
        );

        // Check if we have a pending request
        let mut pending = self.pending_sync_request.lock().unwrap();
        let pending_request = match pending.take() {
            Some(req) if req.peer == response.responder => req,
            Some(req) => {
                // Got response from different peer, ignore but restore pending
                *pending = Some(req);
                return Ok(());
            }
            None => {
                // No pending request, ignore
                return Ok(());
            }
        };

        drop(pending);

        if response.blocks.is_empty() {
            return Err(format!(
                "Peer {:?} has no blocks for requested range {}-{}",
                &response.responder[..4],
                pending_request.start_height,
                pending_request.end_height
            ));
        }

        let mut db = self.db.lock().unwrap();
        let state = self.state.lock().unwrap();
        let committed_height = state.committed_height;

        // Check that first block connects to our committed chain
        if !response.blocks.is_empty() && response.blocks[0].height != committed_height + 1 {
            return Err(format!(
                "Block chain gap: committed_height={}, first block height={}",
                committed_height, response.blocks[0].height
            ));
        }

        // Get expected parent hash
        let expected_parent_hash = if committed_height > 0 {
            let committed_block = ConsensusState::load_block(&*db, committed_height)
                .map_err(|e| format!("Failed to load committed block: {:?}", e))?
                .ok_or_else(|| "Missing committed block".to_string())?;
            novai_consensus_types::block_hash(&committed_block)
        } else {
            [0u8; 32] // Genesis parent
        };

        drop(state);

        // Verify the blocks form a valid chain
        if let Err(e) = ConsensusState::verify_block_chain(&response.blocks, expected_parent_hash) {
            return Err(format!("Block chain verification failed: {:?}", e));
        }

        // Store blocks to DB
        for block in &response.blocks {
            let key = novai_state::block_key(block.height);
            let value = novai_consensus_types::codec::encode_block_v1(block)
                .map_err(|e| format!("Failed to encode block: {:?}", e))?;
            db.put(&key, &value)
                .map_err(|e| format!("Failed to store block: {:?}", e))?;
        }

        drop(db);

        // Update committed_height in state
        if let Some(last_block) = response.blocks.last() {
            let mut state = self.state.lock().unwrap();
            state.committed_height = last_block.height;

            println!(
                "✅ Synced to height {} ({} blocks applied)",
                last_block.height,
                response.blocks.len()
            );
        }

        Ok(())
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

        // Reset round timer - we received a valid proposal
        *self.round_start_time.lock().unwrap() = Instant::now();
        *self.timed_out_this_round.lock().unwrap() = false;

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

        // Log AI signal if present (advisory only)
        if let Some(commitment) = vote.ai_signal_commitment {
            println!("📊 Node received vote with AI signal: {:?}", commitment);
        }

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
                .persist_commit_atomic(&mut *db, &to_commit, &qc, new_committed_height, None)
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
            NetworkMessage::Timeout(t) => self.handle_timeout(t).map(|_| ()),
            NetworkMessage::BlockRequest(req) => self.handle_block_request(req),
            NetworkMessage::BlockResponse(resp) => self.handle_block_response(resp),
        }
    }
}
