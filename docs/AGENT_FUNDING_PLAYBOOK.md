# NOVAI Agent Funding Playbook

Reusable two-key Type-10 funding lifecycle. Every NOVAI sub-agent past
the price-oracle (agent #1) copies this verbatim and only varies the
per-agent block at the bottom. The playbook supersedes the single-key
Type-8 model the price-oracle was originally shipped with; that model
failed because the creator address is bound in the chain-side reverse
index at registration, which prevents the creator from later sending
`CreditAiEntity` to fund the entity it created.

Source diagnosis: `docs/gate-oracle-funding-model-diagnosis.md` (Gate 3
analysis against `crates/execution/src/lib.rs:9701-9742` and the SDK at
`sdk/novai-python-sdk/novai_sdk/client.py:703-726`). Smoke runbook:
`docs/gate-oracle-two-key-smoke-test-runbook.md`. Reference agent
implementing the model: `agents/price-oracle/`.

## Critical warning: fresh funder per agent identity

The chain derives `entity_id` from `(code_hash, funder_address)`, not
from the entity signing pubkey. See
`crates/execution/src/lib.rs:9378-9379`. Two agents sharing the same
`code_hash` collide at `EntityAlreadyExists` unless they have distinct
funder addresses.

If your agent shares `ORACLE_CODE_HASH` (or any other published
`code_hash`) with an existing agent, you MUST generate a fresh funder
address before bootstrapping. Reusing a funder is the one operational
rule the chain quietly rejects at registration without further
explanation. Treat "funder address" the same way you treat "private
signing key": never share, never reuse across agent identities.

The corollary: if your agent uses a unique `code_hash`
(`blake3(b"novai-<agent-name>-v1")`), funder reuse is safe in principle
but still discouraged: a per-agent funder keeps blast-radius narrow if
the funder seed is ever compromised.

## The two-key model in thirty seconds

- **Funder keypair** (ed25519 account). Signs the Type-10 registration
  tx, pays the registration fee, seeds the entity's `economic_balance`,
  then later signs `CreditAiEntity` txs to top up the entity. NEVER
  signs entity-bound signals (`OracleAnchor`, memory CRUD, etc.);
  doing so routes through `check_ai_entity_sender`'s deny arm at
  `lib.rs:9741`.
- **Entity keypair** (ed25519 entity signing key). Signs
  `SignalCommitment` txs only. Holds the per-agent capability byte
  (`Capabilities::oracle() = 0x47` for price-oracle, different for
  other agent types). Never holds spendable account balance; the
  entity's `economic_balance` ledger is debited by signal fees and
  credited by funder-signed `CreditAiEntity` txs.

The funder address is never written into the chain's
`ai_entities_by_addr` reverse index; only the entity-pubkey-derived
address is bound at registration. `lookup_ai_entity_by_address(funder)`
returns `None`, so the funder is on a non-entity-bound path and
`CreditAiEntity` from the funder clears the deny gate.

## Lifecycle (eight steps, identical for every agent)

1. **Load or generate the funder ed25519 keypair.** Confirm the funder
   address is not already entity-bound for your `code_hash` via
   `chain.funder_is_unbound(funder.address)`. False return means the
   funder is poisoned; refuse to proceed and rotate.

2. **Faucet the funder address.** The funder needs at least
   `initial_balance + register_fee + safety_margin` to cover the
   Type-10 registration plus operational headroom. The faucet RPC at
   `crates/node/src/rpc.rs:3129` is per-IP / per-24h-cooldown; size
   `initial_balance` so a single faucet drop suffices.

3. **Load or generate the entity ed25519 keypair.** This key never
   holds spendable balance and never signs anything other than
   entity-scoped operations (signals, memory CRUD, optional
   `Transfer` if your capability preset includes it).

4. **Submit Type-10 `RegisterAiEntityWithKey` signed by the funder.**
   Payload: `code_hash`, `entity_pubkey`, `autonomy_mode = GATED`,
   per-agent `capabilities` preset, `initial_balance` sized for
   first-N business operations plus buffer. SDK surface:
   `client.register_entity_with_key(...)` at
   `sdk/novai-python-sdk/novai_sdk/client.py:703-726`.

5. **Verify on-chain.** Poll `chain.get_entity_status(entity_id)` until
   `exists` is True with the expected `capabilities` byte and
   `economic_balance >= initial_balance`. Time out and exit non-zero
   on persistent failure.

6. **Persist the funder seed, the entity seed, and the `entity_id` to
   a versioned keyfile at 0600.** The keyfile is the only persistent
   secret. Re-running the bootstrap on a host with a populated keyfile
   is a no-op (idempotent skip path on
   `status.exists and status.has_<capability>`).

7. **Runtime top-up loop (in the agent's long-running process).**
   On every cycle, read `entity.economic_balance` via
   `chain.get_entity_economic_balance(entity_id)`. If below a per-agent
   threshold, sign a `CreditAiEntity` with the funder keypair using
   `chain.get_account_nonce(funder.address)` for the strict-equality
   nonce, and submit. If already sufficient, skip.

8. **Business signal submission.** Signed by the entity keypair. For
   oracles: `post_oracle_anchor`. For other agent types: the
   corresponding signal builder. The entity nonce is range-checked
   (not equality-checked) at the chain layer, so a single-flight loop
   does not need account-style nonce management here.

## Per-agent variation block

Only the following differ per agent; everything above stays identical.

- **`code_hash`** differs per agent type. Convention:
  `blake3(b"novai-<agent-name>-v1")`. Bumping the trailing version is a
  deliberate semantic change that produces a new `entity_id` and
  forces a fresh registration.
- **`capabilities` preset** differs per agent type. Oracle uses
  `Capabilities::oracle()` (`0x47`, bits 0,1,2,6:
  `read_public_chain`, `read_memory_objects`, `emit_proposals`,
  `post_oracle_anchors`). Pick the preset that matches your bit set;
  capabilities are frozen post-register and cannot be upgraded.
- **Business-signal call** differs. Oracles call `post_oracle_anchor`;
  other agent types call their primary signal builder.
- **Economic data source** differs. Price oracle reads CoinGecko;
  the next oracle might read a different feed; non-oracle agents skip
  this stage entirely.
- **Top-up threshold and credit amount** differ per agent based on
  the expected fee burn rate. Size so the entity never falls below the
  per-signal minimum fee between top-up cycles.

## Operational rules

- **Funder address must be fresh per (agent_type, agent_identity)
  tuple** when the agent shares its `code_hash` with another agent.
  The chain rejects collisions at `EntityAlreadyExists` (`lib.rs:9382`).
- **The funder address must never be entity-bound** (creator of any
  AI entity under any `code_hash`). The chain's
  `check_ai_entity_sender` denies `CreditAiEntity` (and Types 6, 7, 8,
  10) from any sender whose address is in `ai_entities_by_addr`.
- **The nonce source for funder-signed txs is
  `chain.get_account_nonce(funder.address)`**, NOT `novai_getNonce`.
  The chain's `apply_credit_ai_entity_tx` uses exact equality against
  `account.nonce`; the mempool's `expected_nonce` advances on every
  committed tx (success or fail) and drifts away from `account.nonce`
  whenever entity-signed signals are interleaved. See
  `lib/chain.py:get_account_nonce` for the documented rationale.
- **The keyfile's `entity_id_hex` is the authority for the agent's
  `entity_id`**, not a runtime derivation from the funder address.
  The bootstrap persists it after registration; the long-running
  process should hard-fail on disagreement between keyfile and
  derivation rather than silently fund the wrong entity.
- **Capabilities are frozen post-register.** Typoing the
  `capabilities` byte at registration time is unrecoverable short of
  abandoning the entity and re-registering under a new funder.

## Failure modes and remediation

| Failure | Where | Remediation |
|---|---|---|
| `EntityAlreadyExists` at Type-10 register | `lib.rs:9382-9384` | Funder collided. Rotate funder to a fresh address. |
| `EntityAlreadyExists` at reverse-index write | `lib.rs:9391-9393` | Entity pubkey collision (practically impossible for random ed25519). Regenerate entity key. |
| `IssuerMissingCapability` on `CreditAiEntity` from funder | `lib.rs:9741` | Funder is entity-bound. Diagnosis: funder was previously used to register an entity. Rotate funder. |
| `NonceMismatch` on `CreditAiEntity` | `lib.rs:9226` | Caller used `novai_getNonce` instead of `get_account_nonce`. Re-source nonce from on-chain `account.nonce`. |
| `InsufficientFunds` on first business signal | signal handler | `initial_balance` too small; the entity drained between birth and first runtime top-up. Increase `INITIAL_ENTITY_BALANCE` for the next agent and re-bootstrap. |
| `EntityNotActive` on signal | various | Entity is governance-disabled or kill-switched. Operator intervention; no code fix. |
| `OracleAnchorAlreadyExists` | `lib.rs:4150-4158` | Replay of an identical signal. Vary `data_hash` or `external_timestamp`. |

None of these are protocol-side blockers; all are operational and
detectable at bootstrap time or in a CI smoke test that replays the
runbook at `docs/gate-oracle-two-key-smoke-test-runbook.md`.

## When to deviate from this playbook

You should not. If your agent does not fit the eight-step shape (e.g.
a passive observer that submits no signals, or an agent that needs to
pay its own fees from spendable account balance), open a design doc
first; the diagnosis assumes the eight-step shape and the chain's
capability model is built around it. Cf. the Q-blocks in the source
diagnosis for the load-bearing assumptions.
