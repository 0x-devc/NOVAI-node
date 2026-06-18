# 535004 Layer 4: the locked-QC safety rule (design spec)

Status: design. No patch code is written yet. This document is the specification
I will implement and test in the next phase. All file references are
repository-relative paths into `crates/consensus/src/lib.rs` unless noted.

## 0. Scope

The implemented consensus can be driven to commit two different blocks at one
height with a single Byzantine validator (n=4, f=1, quorum=3). The executable
proof is the regression test `two_conflicting_commits_via_qc_migration_535004`
in `crates/consensus/src/lib.rs`, which fails on the current tree at the safety
assertion: two honest nodes commit different blocks at height 1.

The root cause is that `ConsensusState` has no lock. `highest_qc` is allowed to
migrate between two conflicting same-height QCs (a round-1 QC replaces a round-0
QC at the same height, with no round reset), and `verify_block` then blesses the
conflicting child because it checks only height and parent against the migrated
`highest_qc`.

This spec adds a standard locked-QC rule: a `locked_qc` field, a single
`safe_to_extend` safety primitive, a SET point at QC adoption (the 1-chain), a
GATE in the QC adoption path and in `verify_block`, and a strict-height UNLOCK
clause. The design is constrained by two prior fixes in the same bug family that
removed a `voted_at_height` rule because it deadlocked legitimate view-change
re-proposals (regression test `view_change_reproposal_not_equivocation`, line
3602). The lock must not reintroduce that halt.

## 1. The locked-QC rule

### 1.1 Field

Add to `ConsensusState` (struct at line 125):

    locked_qc: Option<QC>

Initialized `None` in `new` (line 166) and reloaded in `recover` (line 2161).
It records the QC that this node is committed to: the highest QC it has adopted
on its own branch. It is monotone in height and is never cleared.

### 1.2 The safety primitive

Design (pseudocode, not the final patch):

    fn safe_to_extend(&self, candidate: &QC) -> bool {
        match &self.locked_qc {
            None => true,                                   // not locked yet
            Some(locked) => {
                candidate.block_hash == locked.block_hash   // same certified block
                || candidate.height > locked.height          // strictly higher height
            }
        }
    }

The primitive returns false only for a candidate that is a different block at a
height less than or equal to the lock height. That is exactly a conflicting QC
at or below the locked height, which (Section 2) only a Byzantine arrangement
can produce.

I deliberately do not include a "candidate descends from the locked block"
clause. A descendant at a higher height is already accepted by the strict-height
clause; a descendant at the same height would have to be the locked block
itself, which the same-block clause already accepts; a block at a lower height
is stale and is never needed for progress. Omitting the descends clause also
avoids a parent-chain walk over `block_by_hash` for a QC whose block this node
may not have cached.

### 1.3 SET (the 1-chain lock)

`locked_qc` advances to `highest_qc` at every safe adoption of `highest_qc`,
that is at the three sites that assign `highest_qc`:

- `cache_qc_and_check_commit`, the commit-path install (line 1290),
- `add_timeout`, the pre-gate self-heal adoption (line 930),
- `add_timeout`, the post-gate adoption (line 1022).

Because adoption is itself gated by `safe_to_extend` (Section 1.4), the adopted
QC is always either the same block or a strictly higher height than the current
lock, so `locked_qc` advances monotonically in height and never regresses.

This is the 1-chain lock: the node locks on a QC the moment it adopts that QC as
`highest_qc`, before it votes any child of that QC. Locking at the 1-chain is
load-bearing. A textbook chained-HotStuff lock is set at the 2-chain (lock on
the grandparent when a QC-of-a-QC is observed). That is one height too late
here: the proven attack has the overlap honest node cast both conflicting middle
votes before any middle QC forms, so a 2-chain lock would be installed after the
damage is done. Section 2.2 depends on the lock being set at adoption.

### 1.4 GATE: QC adoption (the migration gate, primary safety mechanism)

At the three adoption sites the existing "dominating QC" test
(`qc.height > existing.height || (qc.height == existing.height && qc.round >
existing.round)`, lines 918, 1010, 1232) is additionally conditioned on
`safe_to_extend(candidate)`. A candidate that does not pass `safe_to_extend` is
not adopted; `highest_qc` keeps its current value.

This is the primary mechanism. It keeps `highest_qc` on the locked branch, so a
conflicting same-height QC can never replace it. With `highest_qc` pinned to the
locked branch, the existing parent-hash check in `verify_block` (line 388)
already rejects a conflicting child on its own.

### 1.5 GATE: voting (`verify_block`)

`verify_block` (line 330) gains a check: refuse the vote unless
`safe_to_extend(highest_qc)` holds, where `highest_qc` is the QC the proposed
block extends (the block's parent is `highest_qc.block_hash`, enforced at line
388). The leader's self-vote on its own proposal
(`crates/node/src/consensus_node.rs:1304`) is covered transitively, because
`propose_block` (line 191) builds on `highest_qc`, which the migration gate
keeps on the locked branch.

In steady state `locked_qc` equals `highest_qc`, so this check is normally
trivially satisfied and is therefore defense in depth rather than the primary
mechanism. I keep it for two reasons: it states the safety rule in the textbook
location (the vote decision), and it catches any path that sets `highest_qc`
without going through the migration gate, for example a `highest_qc` reloaded
from disk in `recover` on a node that crashed mid-attack. I will state plainly
in the implementation that the migration gate is what defeats the attack and the
`verify_block` gate is the explicit, redundant safety statement.

### 1.6 UNLOCK

There is no separate unlock operation. The lock moves forward, it is never
released. The `candidate.height > locked.height` clause in `safe_to_extend` is
the unlock: when a node sees a QC at a strictly higher height it adopts it and
re-locks there, even if that QC is on a different branch. Section 2.3 proves a
Byzantine minority branch can never reach a height above an honest node's lock,
so this clause is only ever satisfied by the genuine majority branch. That is
what makes the unlock safe for safety and sufficient for liveness at the same
time.

### 1.7 Persistence and the no-clear invariant

`locked_qc` is persisted under a new key `KEY_LOCKED_QC`, written alongside
`highest_qc` (today `highest_qc` is persisted by `persist_highest_qc` at line
1693 and inside `persist_commit_atomic` at line 1821), and reloaded in `recover`
(line 2161) the way `highest_qc` is loaded by `load_highest_qc` (line 1885). A
node that crashes mid-attack and reloads `highest_qc` must also reload its lock,
or it could vote a conflicting block after restart. The test file already
anticipates this: `crates/consensus/tests/chaos_crash.rs:301` comments that
recovery "also loads latest QC, locked_round, etc."

`locked_qc` must NOT be cleared by any of the round/state resets:

- the view-change reset on a strict height advance (lines 1274 to 1287),
- the post-commit reset (lines 1506 to 1518),
- the round-sync partial clear in `add_timeout` (lines 1040 to 1043).

Those clear round-scoped state (round, pending votes, timeouts). The lock is a
safety invariant that must survive all round churn; if a view change could clear
it, the attack reopens.

## 2. Safety

### 2.1 Walkthrough of the exact proven schedule

I follow the schedule the regression test drives. n=4, f=1, quorum=3. V0, V1, V2
honest; V3 Byzantine. Branch A is at round 0, branch A-prime at round 1, and
they conflict at height 1 (B_1 against B'_1, both extending genesis).

Without the lock (current tree, what the test demonstrates):

1. V0 votes B_1 and B'_1 at height 1 (no lock; both extend genesis). Both
   height-1 QCs form: QC_A(h1) = {V0,V1,V3}, QC_Ap(h1) = {V0,V2,V3}.
2. V0 adopts QC_A(h1) as `highest_qc` and votes B_2 (branch A middle).
3. V0 is fed QC_Ap(h1). The dominating rule adopts it because it is the same
   height and a higher round, with no reset (the reset at line 1265 is gated on
   a strict height increase). `highest_qc` is now QC_Ap(h1).
4. `verify_block(B'_2)` passes, because the migrated `highest_qc` makes B'_2 the
   expected child. V0 votes B'_2. QC_Ap(h2) = {V0,V2,V3} forms.
5. Both branches reach a height-3 QC. V1 commits B_1, V2 commits B'_1.
   B_1 != B'_1. Safety violated.

With the lock:

- At step 2, when V0 adopts QC_A(h1), SET advances `locked_qc` to QC_A(h1)
  (height 1, branch A).
- At step 3, V0 is fed QC_Ap(h1). The migration gate evaluates
  `safe_to_extend(QC_Ap(h1))` against `locked_qc` = QC_A(h1):
  block hashes differ (B'_1 against B_1), and height 1 is not greater than the
  lock height 1. The result is false. This is the precise line where the attack
  dies: the adoption at the commit-path install (line 1290, now guarded by
  `safe_to_extend`) is skipped, so `highest_qc` stays QC_A(h1).
- At step 4, `verify_block(B'_2)` now fails: the expected parent is
  QC_A(h1).block_hash (B_1), but B'_2's parent is B'_1. The lock gate in
  `verify_block` would reject it too. V0 does not vote B'_2.
- QC_Ap(h2) needs three distinct voters. Only V2 and V3 remain on branch
  A-prime; that is two, below quorum. QC_Ap(h2) never forms. Branch A-prime is
  starved at the middle. It never reaches a height-3 QC, so V2 never commits at
  height 1.

Tie to the test variables: with the lock,
`v0.cache_qc_and_check_commit(qc_ap_h1, &db)` does not migrate, so `v0.highest_qc`
stays `qc_a_h1`; `gated(&v0, &b2p)` calls `verify_block(b2p)`, which fails the
parent check and returns `None`, so `v0_b2p` is `None`; `ap_h2_votes` is
{V2, V3}, length 2, below quorum, so `assemble_qc(...)` returns `None` and
`qc_ap_h2` is `None`; therefore `qc_ap_h3` is `None`; therefore `v2_h1` is
`None`; the final `if let Some(h_v2) = v2_h1` is skipped and the safety
assertion never runs. The harness-sanity assertion (V1 commits B_1) still holds,
because branch A is untouched. The test passes.

### 2.2 General safety theorem

Claim. Under the locked-QC rule, for any behavior of a single Byzantine
validator in an n=4, f=1 system, no two honest nodes commit conflicting blocks
at the same height. This is general over all Byzantine arrangements, not the one
schedule above.

Proof. Suppose for contradiction two honest nodes commit conflicting blocks B
and B' at height h, with B != B'.

1. By the 3-chain commit rule (`cache_qc_and_check_commit`, commit target is
   QC height minus 2, lines 1296 to 1301), committing height h on a branch
   requires a QC at height h+2 on that branch whose block chains back through
   h+1 to h. For the height h+2 block to have been proposed and voted, it had to
   extend a QC at height h+1 (the proposer builds on `highest_qc` at line 252,
   and honest voters require the parent to match `highest_qc` at line 388). So a
   commit of height h on a branch implies a QC at height h+1 on that branch.
   Therefore both branches have a QC at height h+1: QC_B(h+1) and QC_Bp(h+1).

2. Each QC has quorum, 2f+1 = 3 distinct voters out of 4. Two sets of size 3 in
   a universe of 4 intersect in at least 3 + 3 - 4 = 2 validators. At most one
   is Byzantine, so at least one HONEST validator H is in the intersection: H
   voted for both B_{h+1} and B'_{h+1}, two conflicting blocks at height h+1.

3. To vote B_{h+1}, H needed `highest_qc` = QC_B(h), because `verify_block`
   requires the parent to equal `highest_qc.block_hash` and B_{h+1}'s parent is
   B_h. To vote B'_{h+1}, H needed `highest_qc` = QC_Bp(h). These are two
   conflicting QCs at the same height h.

4. H is honest, so it ran the rule. Consider the first of QC_B(h), QC_Bp(h) that
   H adopted. By SET (Section 1.3), at that adoption H set `locked_qc` to a QC at
   height h on that branch. When H later met the conflicting height-h QC on the
   other branch, the migration gate evaluated `safe_to_extend` against a lock at
   height h: the candidate is a different block and its height h is not greater
   than the lock height h, so the result is false and H did not adopt it.
   Adoption is sequential under the state lock, so H could not have adopted both
   at once. Therefore H held only one of QC_B(h), QC_Bp(h) as `highest_qc`, and
   could vote only one of B_{h+1}, B'_{h+1}. This contradicts step 2.

5. The contradiction holds for either adoption order and for any assignment of
   the Byzantine validator and any rounds the adversary chooses, because steps 1
   and 2 use only the commit rule and quorum intersection, and step 4 uses only
   the 1-chain SET and `safe_to_extend`. Hence no two honest nodes commit
   conflicting blocks at height h. QED.

The argument generalizes to any n = 3f+1: two quorums of size 2f+1 intersect in
at least f+1 validators, of which at least one is honest, and the same lock step
blocks that honest overlap voter. So the fix is not specific to n=4 or to the
demonstrated schedule.

Note on what the lock does and does not prevent. The lock does NOT prevent two
QCs from forming at the divergence height itself (height 1 in the walkthrough):
no node is locked there yet, so the overlap honest node legitimately votes both.
What the lock prevents is propagation: the overlap node locks at the divergence
height upon adopting the first QC there, so it cannot vote the conflicting child
at divergence+1, and without it the conflicting branch is sub-quorum at
divergence+1 and can never form a 3-chain. The safety property is "no two
conflicting commits," not "no two same-height QCs"; the latter is false at the
divergence height and need not hold.

### 2.3 Supporting lemma and the precise unlock claim

Lemma (participation count). Under n = 3f+1, a QC and a round advance cannot both
occur at the same (height, round). A QC needs 2f+1 votes; a round advance needs
2f+1 timeouts (`try_advance_round`, line 1063). An honest node casts at most one
of a vote or a timeout per (height, round). With at least 2f+1 honest nodes
contributing one each and at most f Byzantine contributing two each, the maximum
participation is (2f+1) + 2f = 4f+1, which is below the 4f+2 needed for both. So
if a QC formed at a round, no 2f+1 round advance happened at that round.

Consequence for same-round conflicts. Two conflicting QCs at the same
(height, round) are impossible without a Byzantine leader and honest double
voting: each round has one deterministic leader (`compute_leader_for_view`, line
825) proposing one block, and `voted_in_round` (lines 535, 624) stops an honest
node voting twice in one round. So a conflict requires different rounds, reached
by a node round-syncing on a Byzantine timeout (`add_timeout`, line 1032). That
cross-round equivocation by the overlap honest node is exactly what the lock
catches in Section 2.2.

Precise unlock claim. Combining the theorem with the unlock clause: a conflicting
(Byzantine minority) branch can never obtain a QC at a height above an honest
node's lock. By Section 2.2, the conflicting branch is starved of the overlap
honest voter at divergence+1, so its highest QC sits at the divergence height,
which equals the lock height, never above it. Therefore the strict-height unlock
(`candidate.height > locked.height`) can never be satisfied by a conflicting
branch under any f=1 arrangement. It is only ever satisfied by a genuinely
higher branch that carries its own honest quorum, which is the majority. This is
why the unlock is safe.

## 3. Liveness

Synchrony assumption: partial synchrony. Before an unknown global stabilization
time (GST) there is no liveness guarantee, which is standard for BFT. After GST
messages are delivered within a bounded delay, a correct leader's proposal
reaches every honest node, the highest QC propagates, and round timeouts
(`timeout_for_round`, exponential backoff, line 54) eventually give a correct
leader a synchronous round.

General argument. `safe_to_extend` returns false only for a different block at a
height less than or equal to the lock height. Three facts make this harmless to
liveness:

1. Honest forward progress is never blocked. A correct leader builds the next
   block on the highest QC, so its proposal extends the locked branch at height
   lock+1. The QC that block extends is the locked QC (same block) or a strictly
   higher one, so `safe_to_extend` returns true. Every honest node votes.

2. The only case the lock refuses is a same-height-or-lower conflicting QC. By
   Section 2.3 that arises only under Byzantine equivocation, never in honest
   operation, so refusing it costs no honest liveness.

3. A locked node always unlocks to a genuinely higher branch. If the network
   abandons a node's branch and progresses elsewhere to a higher height, that
   branch produces a QC above the node's lock, which `safe_to_extend` accepts
   via the strict-height clause; the node adopts it and re-locks. Because a
   conflicting minority branch can never out-height the lock (Section 2.3), the
   unlock is available exactly when progress is real and never when it is an
   attack. No permanent deadlock.

The lock is not cleared by round advance, round-sync, view-change reset, or
commit reset (Section 1.7), so round churn never disturbs it, and it never
blocks a higher-height adoption.

### 3.1 Case (a): legitimate view-change re-proposal (test 3602)

Scenario: at height H the round-0 leader proposes B_H^{r0}, round 0 fails with no
QC, and the round-1 leader re-proposes B_H^{r1}, a different hash at the same
height extending the same parent (the height H-1 QC). Honest nodes must accept
the re-proposal, or the leader wedges. The earlier `voted_at_height` rule failed
exactly here and was removed.

Why the lock allows it: since no QC formed at height H, no node is locked at
height H (a lock at height H requires adopting a QC at height H). The node's lock
sits at height H-1 or lower. The QC the re-proposal extends is the height H-1 QC.
If that equals `locked_qc`, the same-block clause returns true; if the lock is
lower, the strict-height clause returns true. Either way the re-proposal is
accepted. The lock never asks "have I already voted at this height"; it asks only
whether the proposal extends the locked branch or a higher one, so it cannot
reproduce the `voted_at_height` halt.

In the n=1 regression test `view_change_reproposal_not_equivocation` (line 3602)
specifically, the test never calls `cache_qc_and_check_commit`, so `locked_qc`
stays `None` and `safe_to_extend` returns true on the `None` arm. The round-1
self-vote is accepted and the test passes unchanged.

### 3.2 Case (b): minority-branch node rejoining the majority

Scenario: node X locked on QC_min at height 5 on a branch the network abandoned;
the majority committed a different branch to height 20. X receives a majority QC
at height 19. `safe_to_extend` evaluates height 19 against lock height 5: 19 is
greater than 5, so true. X adopts the majority QC as `highest_qc`, and SET
advances `locked_qc` to it. X then votes the next majority proposal at height 20,
which extends that QC (now the lock), so `safe_to_extend` returns true. X has
rejoined with no deadlock. The majority QC at height 19 is real (the majority
formed it with its own honest quorum); by Section 2.3 no Byzantine minority could
have produced a height-19 QC to unlock X onto a fork, so this rejoin path is not
an attack vector.

## 4. Exact touch-points

- Field: `locked_qc: Option<QC>` in `ConsensusState` (struct line 125), init in
  `new` (line 166).
- Primitive: `safe_to_extend(&self, candidate: &QC) -> bool` (Section 1.2), a
  read-only method.
- SET at the 1-chain: advance `locked_qc` to the adopted QC at the three
  `highest_qc` assignment sites, line 1290 (`cache_qc_and_check_commit`), line
  930 and line 1022 (`add_timeout`).
- GATE adoption (migration gate, primary): condition the dominating-QC adoption
  on `safe_to_extend` at lines 918/930, 1010/1022, and 1232/1290.
- GATE voting: add a `safe_to_extend(highest_qc)` check in `verify_block` (line
  330), covering the received-proposal vote path
  (`crates/node/src/consensus_node.rs:1610`); the leader self-vote
  (`consensus_node.rs:1304`) is covered transitively via a clean `highest_qc`.
- UNLOCK: the `candidate.height > locked.height` clause in `safe_to_extend`; no
  separate operation.
- Persistence: new `KEY_LOCKED_QC`, written next to `highest_qc` in
  `persist_commit_atomic` (line 1821) and a `persist_locked_qc` mirroring
  `persist_highest_qc` (line 1693); reloaded in `recover` (line 2161) mirroring
  `load_highest_qc` (line 1885).
- No-clear: `locked_qc` is excluded from the resets at lines 1274 to 1287, 1506
  to 1518, and 1040 to 1043.

## 5. Regression risk by category

VIEW-CHANGE and ROUND-ADVANCE. Round advances form no QC, so they never hit the
migration gate, and the lock is not cleared by round changes. The re-proposal
test 3602 passes (Section 3.1). The self-heal-on-timeout test
`add_timeout_early_adoption_self_heals_wrong_view` (line 3718) adopts a QC that
is the first QC or a higher view, which `safe_to_extend` accepts, so it passes.
Risk: low.

BYZANTINE and EQUIVOCATION. The lock is additive: it adds refusal of cross-branch
same-height migration and conflicting votes; it does not change duplicate
detection. `within_round_duplicate_still_rejected_on_both_paths` (line 3659)
exercises `voted_in_round` on `add_vote` and `add_vote_verified`, which the lock
does not touch, so it passes. The chaos byzantine and partition safety tests
(`test_safety_under_byzantine_faults`, `test_byzantine_minority_cannot_fork`,
`test_conflicting_proposals`, `test_safety_property_no_conflicting_commits`)
assert safety holds; the lock can only strengthen them. Risk: low, with one watch
item: if a chaos scenario relied on the buggy same-height migration to make
progress, the lock would change its path. I will watch these run in the
implementation phase.

RECOVERY and CRASH. Adding a field changes `new` and `recover` and any
struct-literal construction of `ConsensusState`. I will grep for struct literals
in the implementation phase; the test helpers I have read use
`ConsensusState::new`, not literals. `recover` must load `locked_qc` (None when
absent) without panicking. Recovery tests assert on `committed_height` and
`highest_qc`, not the new field, so they should pass once `recover` is updated.
Risk: medium on compilation (the field must be threaded through every
constructor), low on behavior.

SAFETY and COMMIT. `commit_rule_3_chain` and `commit_rule_batch_commits` drive a
single clean branch; the lock advances monotonically and never refuses, so they
pass. The dense-QC and persist tests use `persist_commit_atomic` and do not
exercise the lock gate; adding `KEY_LOCKED_QC` persistence does not affect their
assertions. Highest watch item: `highest_qc_updated` (in
`crates/consensus/tests/consensus_basic.rs:381`). If that test asserts a
same-height-higher-round QC replaces `highest_qc`, the migration gate now refuses
that replacement when the blocks conflict, and the test would need updating
because it codified the buggy behavior. I will read that test in full before
touching code and report whether it needs an update or already uses a
non-conflicting higher-height QC. Risk: medium, isolated to that one test.

## 6. What this spec does not do, and open items

- It does not change the commit rule, the QC formation rule, or the leader
  schedule. It adds a lock and two gates.
- It does not address liveness before GST; partial synchrony is assumed, as for
  the existing protocol.
- Open item to resolve during implementation, not now: confirm whether
  `highest_qc_updated` (consensus_basic.rs:381) asserts a conflicting
  same-height migration; if so it documented buggy behavior and will be updated
  to reflect the lock, with that change called out explicitly.
- Open item: confirm the full set of `ConsensusState` constructors and struct
  literals so the new field is threaded everywhere and the crate compiles before
  any behavioral test is run.
