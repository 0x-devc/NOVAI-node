//! W4: the gate 9 propose-path refusal must not spin the journal.
//!
//! Measured, not hypothetical: a node stuck in the gate 9 anti-equivocation
//! refusal emits about 10 WARN lines per second (the propose tick is 5 ms and
//! the condition holds until the view moves), about 860,000 lines/day against
//! the 13,788 lines/hour budget that buys four days of journald retention. So
//! the node collapses its own retention from days to hours at exactly the
//! moment the journal is the only record of the fault. Three root causes have
//! already been lost in this project to expired journals.
//!
//! The rule this gate pins is narrower than "log less": keep the FIRST
//! occurrence, keep EVERY view transition, suppress only the repetition. A
//! climbing round while commits are frozen is the livelock signature and the
//! single most diagnostic fact in the stream, so it must be unsuppressible.
//! Suppressing per VIEW rather than per time window is what buys that
//! property, and it is the mechanism `every_view_transition_logs` exists to
//! defend: an unconditional suppressor still passes a "logs less" test and
//! fails this one.
//!
//! These tests drive the REAL production path (`try_propose_block`) and read
//! the REAL WARN stream through a `tracing` subscriber, so nothing here can
//! pass against a re-implementation of the decision.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_state::{Kv, KEY_VOTED_VIEW};
use novai_types::Address;
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

struct TestNonceProvider;

impl mempool::NonceProvider for TestNonceProvider {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0
    }
}

/// Sink for the production log stream. The gate asserts on what an operator
/// would actually read in journald, not on an internal counter.
#[derive(Clone, Default)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl CapturedLog {
    /// Only the gate 9 refusal lines; the path emits other records and the
    /// budget question is about this one.
    fn gate9_lines(&self) -> Vec<String> {
        let raw = self.0.lock().expect("log buffer");
        String::from_utf8(raw.clone())
            .expect("log output is utf8")
            .lines()
            .filter(|l| l.contains("gate 9:"))
            .map(str::to_string)
            .collect()
    }
}

impl Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn make_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            let sk = SigningKey::from_bytes(&[i as u8; 32]);
            let pk = sk.verifying_key();
            (address_from_pubkey(&pk), sk, pk)
        })
        .collect()
}

/// A node parked in the gate 9 refusal on the propose path.
///
/// With height 0 and no highest QC the intended view is (1, state.round), and
/// committed_height 0 keeps that inside the commit window, so the commit-window
/// refusal above gate 9 does not fire and gate 9 is the branch under test. A
/// durable mark at (1, 5) makes `may_vote` false for every round below 5, which
/// is what lets the gate hold the refusal fixed while the ROUND advances: that
/// is the restart-replay shape in production, and the only shape where the
/// difference between per-view and unconditional suppression is observable.
fn parked_node() -> ConsensusNode {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let mut pubkeys = HashMap::new();
    for (a, _, pk) in &validators {
        pubkeys.insert(*a, *pk);
    }
    let node = ConsensusNode::new(validators[0].1.clone(), validator_set, pubkeys, 1000);
    {
        let mut state = node.state.lock().unwrap();
        state.voted_view = Some((1, 5));
    }
    node
}

fn subscriber(log: &CapturedLog) -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish()
}

/// Fire the propose tick `ticks` times at the current view. Every call must be
/// the quiet Ok(false) skip: W4 changes what is logged, never what is decided.
fn tick(node: &ConsensusNode, pool: &mut mempool::TxMempool, ticks: usize) {
    for _ in 0..ticks {
        let proposed = node
            .try_propose_block(pool, &TestNonceProvider)
            .expect("the gate 9 refusal is a quiet skip, never an error");
        assert!(
            !proposed,
            "gate 9 must still refuse to propose at an already-voted view"
        );
    }
}

fn set_round(node: &ConsensusNode, round: u64) {
    node.state.lock().unwrap().round = round;
}

#[test]
fn repeated_refusal_at_one_view_logs_once() {
    let log = CapturedLog::default();
    let node = parked_node();
    let mut pool = mempool::TxMempool::new(1, 100);

    tracing::subscriber::with_default(subscriber(&log), || {
        tick(&node, &mut pool, 200);
    });

    let lines = log.gate9_lines();
    assert_eq!(
        lines.len(),
        1,
        "200 refusal ticks at ONE view must emit exactly one gate 9 line \
         (at 5 ms per tick an unsuppressed line is ~10/s, ~860k/day, against a \
         13,788 lines/hour retention budget); got {}:\n{}",
        lines.len(),
        lines.join("\n")
    );
}

#[test]
fn every_view_transition_logs() {
    // The mechanism gate. A rate limit that can swallow a view change hides the
    // climbing round, which is the livelock signature, and is worse than no rate
    // limit at all. Three dwelling views, three lines, in round order.
    let log = CapturedLog::default();
    let node = parked_node();
    let mut pool = mempool::TxMempool::new(1, 100);

    tracing::subscriber::with_default(subscriber(&log), || {
        tick(&node, &mut pool, 50);
        set_round(&node, 1);
        tick(&node, &mut pool, 50);
        set_round(&node, 2);
        tick(&node, &mut pool, 50);
    });

    let lines = log.gate9_lines();
    assert_eq!(
        lines.len(),
        3,
        "each of the three views must log exactly once: the first occurrence \
         and every transition survive, only repetition is suppressed; got {}:\n{}",
        lines.len(),
        lines.join("\n")
    );
    for (i, round) in [0u64, 1, 2].iter().enumerate() {
        assert!(
            lines[i].contains(&format!("round={round}")),
            "line {i} must report round={round}, the fact the operator needs to \
             see the round climbing while commits are frozen; got: {}",
            lines[i]
        );
        assert!(
            lines[i].contains("height=1"),
            "line {i} must still carry the intended height; got: {}",
            lines[i]
        );
    }
}

#[test]
fn emitted_line_carries_the_suppressed_count() {
    // While suppressed the operator cannot see that the condition is still
    // active, so the next emitted line reports how many ticks were swallowed
    // since the previous one. That count is the dwell in the previous view, and
    // it is what turns "one line per view" back into a measurable duration.
    let log = CapturedLog::default();
    let node = parked_node();
    let mut pool = mempool::TxMempool::new(1, 100);

    tracing::subscriber::with_default(subscriber(&log), || {
        // 1 emitted plus 49 swallowed at (1, 0).
        tick(&node, &mut pool, 50);
        set_round(&node, 1);
        tick(&node, &mut pool, 1);
    });

    let lines = log.gate9_lines();
    assert_eq!(lines.len(), 2, "one line per view; got:\n{}", lines.join("\n"));
    assert!(
        lines[0].contains("suppressed_ticks=0"),
        "the first line swallowed nothing before it; got: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("suppressed_ticks=49"),
        "the transition line must report the 49 ticks swallowed at the previous \
         view, so the operator can price the dwell; got: {}",
        lines[1]
    );
}

#[test]
fn suppression_does_not_touch_the_refusal_or_the_durable_mark() {
    // W4 is a logging change. The refusal must refuse in exactly the cases it
    // refused before, and nothing on the durable vote path may move.
    let log = CapturedLog::default();
    let node = parked_node();
    let mut pool = mempool::TxMempool::new(1, 100);

    tracing::subscriber::with_default(subscriber(&log), || {
        tick(&node, &mut pool, 20);
        set_round(&node, 1);
        tick(&node, &mut pool, 20);
    });

    {
        let state = node.state.lock().unwrap();
        assert_eq!(
            state.voted_view,
            Some((1, 5)),
            "a gate 9 refusal must leave the durable high-water mark exactly where it was"
        );
        assert_eq!(
            state.last_proposed, None,
            "a refused proposer must not burn its proposal slot"
        );
    }
    {
        let db = node.db.lock().unwrap();
        assert_eq!(
            db.get(KEY_VOTED_VIEW).unwrap(),
            None,
            "a gate 9 refusal writes no durable vote mark"
        );
    }
}
