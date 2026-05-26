# NOVAI Python SDK

Pure-Python client for the [NOVAI](https://github.com/0x-devc/NOVAI-node) blockchain.

`novai-sdk` lets agent-framework code (LangChain, CrewAI, AutoGen, custom Python tools) talk to a NOVAI node without any Rust toolchain. It wraps the full JSON-RPC surface, signs transactions locally with ed25519 (via PyNaCl), and constructs every payload byte-for-byte the way the Rust node expects.

```python
from novai_sdk import NOVAIClient, Keypair, Capabilities, PaymentCondition

client = NOVAIClient("http://localhost:3030")
kp = Keypair.load("oracle.key")

services = client.discover_services(category="inference")
result = client.pay(
    keypair=kp,
    issuer_entity_id=my_entity_id,
    payee=services[0].entity_id,
    amount=5000,
    signal_hash=request_id,
    service_descriptor_hash=services[0].object_id,
    request_hash=request_id,
    max_block_height=client.get_latest_block().height + 100,
    condition=PaymentCondition.anchor_exists(anchor_hash),
)
print(f"paid: txid={result.txid}")
```

## Install

```bash
pip install novai-sdk
```

Python 3.9+. Depends on `pynacl`, `blake3`, and `aiohttp`. No Rust toolchain required.

## What's covered

| Feature                       | Tx / signal types         | Notes                                             |
| ----------------------------- | ------------------------- | ------------------------------------------------- |
| Transfers                     | tx 1                      | `client.transfer(...)`                            |
| AI entity lifecycle           | tx 8, 9, 10               | register / register-with-key / credit             |
| Entity upgrades (Week 34)     | tx 11                     | `client.upgrade_entity(...)`                      |
| Memory CRUD                   | tx 3, 4, 5                | generic + typed encoders for 4 high-level types   |
| Governance                    | tx 6, 7                   | submit / execute proposal                         |
| Signal commitment             | tx 2                      | `client.publish_signal(...)`                      |
| Reputation / stake / proofs   | signal 7..=13             | per-type extras encoders in `novai_sdk.signals`   |
| Subscriptions                 | signal 14, 15             |                                                   |
| Payments (Week 28)            | signal 16, 17             | `client.pay(...)`, `client.attest_payment(...)`   |
| Multi-party splits (Week 33)  | signal 16 trailer         | `splits=[PaymentSplit(...), ...]`                 |
| SLAs (Week 31)                | memory 14 + signal 18     | `client.accept_sla(...)`                          |
| Payment channels (Week 32)    | memory 15 + signal 19/20/21 | `client.accept_channel / close_channel / finalize_channel` |
| Off-chain channel signing     | (no chain ops)            | `sign_channel_state(...)` in `novai_sdk.crypto`   |
| Oracle anchors (Week 35)      | signal 22                 | `client.post_oracle_anchor(...)`                  |
| Conditional execution (Week 36)| signal 16 trailer        | `condition=PaymentCondition.anchor_exists(...)`   |
| Service discovery (Week 29)   | memory 12                 | `client.discover_services(category=...)`          |
| VK registry (Week 30)         | memory 13                 | `client.list_vk_registrations(...)`               |
| Range queries                 | RPC paginators            | `iter_signals_by_issuer`, `iter_payments_by_entity` (10K-block auto-chunk) |

## Quick start

### 1. Make a key

```python
from novai_sdk import Keypair

kp = Keypair.generate()
kp.save("my.key")  # raw 32-byte seed, CLI-compatible

print(f"address: {kp.address_hex}")
print(f"pubkey:  {kp.pubkey_hex}")
```

The file is binary-compatible with `novai-cli keygen`: you can load a CLI-generated key in Python and vice versa.

### 2. Connect, fund, transfer

```python
from novai_sdk import NOVAIClient, Keypair

client = NOVAIClient("http://localhost:3030")
alice = Keypair.load("alice.key")
bob_addr = bytes.fromhex("...")

# Faucet (dev / testnet only).
result = client.faucet(alice.address)
print(f"faucet: {result.txid}, amount: {result.amount}")

# Send a transfer.
tx = client.transfer(alice, bob_addr, amount=1_000)
print(f"transfer: {tx.txid}")
```

### 3. Register an AI entity

```python
from novai_sdk import Capabilities, AutonomyMode

code_hash = bytes.fromhex("...")  # blake3 of your module code/weights
result = client.register_entity(
    keypair=alice,
    code_hash=code_hash,
    capabilities=Capabilities.oracle(),  # includes post_oracle_anchors
    autonomy_mode=AutonomyMode.GATED,
    initial_balance=1_000_000,
)
print(f"entity_id: {result.entity_id}")
```

The entity ID is derived deterministically as `blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator_address)`; the SDK returns it in `result.entity_id` so the caller doesn't have to recompute it.

To attach an independent signing key to the entity (so it can sign its own signals), use `register_entity_with_key(...)` with a second keypair.

### 4. Post an oracle anchor

```python
result = client.post_oracle_anchor(
    keypair=alice,
    issuer_entity_id=entity_id,
    data_hash=bytes.fromhex("ab" * 32),   # blake3 of the off-chain data
    external_timestamp=1735776000,
    data_tag="price/ETH-USD",             # 1..=32 bytes
    expiry_height=current_height + 1000,  # advisory
)
print(f"anchor: signal_hash={result.signal_hash}")
```

The signal hash is content-addressed (deterministic from the inputs) and the chain rejects duplicates. The SDK derives it locally so the caller never has to know about the derivation.

### 5. Pay another agent with splits and a condition

```python
from novai_sdk import PaymentSplit, PaymentCondition

result = client.pay(
    keypair=alice,
    issuer_entity_id=alice_entity_id,
    payee=primary_recipient_id,
    amount=10_000,
    signal_hash=request_id,
    service_descriptor_hash=service_id,
    request_hash=request_id,
    max_block_height=current_height + 100,

    # Week 33: split across multiple recipients (must sum to 10_000 BPS).
    splits=[
        PaymentSplit(recipient_entity_id=primary_recipient_id, basis_points=7000),
        PaymentSplit(recipient_entity_id=operator_id, basis_points=3000),
    ],

    # Week 36: only release if the anchor's data hash matches.
    condition=PaymentCondition.anchor_data_hash_equals(
        anchor_signal_hash=anchor_id,
        expected_data_hash=expected_hash,
    ),
)
```

Validation runs client-side first (splits must sum to exactly 10_000, primary recipient comes first, no duplicates) so authoring errors raise `ValueError` before the chain sees them. The four condition kinds are all available as `PaymentCondition.anchor_exists` / `anchor_data_hash_equals` / `anchor_tag_equals` / `anchor_not_expired`.

### 6. Open and close a payment channel

```python
# Party A proposes a channel on-chain (writes a memory object).
from novai_sdk.memory_objects import encode_payment_channel

data = encode_payment_channel(
    party_a_entity_id=alice_entity_id,
    party_b_entity_id=bob_entity_id,
    deposit_a=100_000,
    dispute_window_blocks=100,
)
proposal = client.create_memory_object(alice, MemoryObjectType.PAYMENT_CHANNEL, data)

# Party B accepts on-chain.
bob_client = NOVAIClient(...)
bob_client.accept_channel(
    bob, party_b_entity_id=bob_entity_id,
    channel_object_id=proposal_object_id, party_a_entity_id=alice_entity_id,
    signal_hash=...
)

# Both parties exchange off-chain signed state updates as the channel runs:
from novai_sdk import sign_channel_state

sig_a = sign_channel_state(
    alice.signing_key,
    channel_object_id=channel_id,
    party_a=alice_entity_id, party_b=bob_entity_id,
    nonce=10, balance_a=90_000, balance_b=110_000,
    is_final=True,
)
# ...exchange sig_a / sig_b out-of-band, then either party submits:
client.close_channel(
    alice, issuer_entity_id=alice_entity_id,
    channel_object_id=channel_id, party_a_entity_id=alice_entity_id,
    channel_nonce=10, balance_a=90_000, balance_b=110_000, is_final=True,
    sig_a=sig_a, sig_b=sig_b,
    signal_hash=...,
)
```

A cooperative close (`is_final=True` with both signatures) settles immediately. A unilateral close (`is_final=False`) opens a dispute window during which the counterparty can submit a higher-nonce close; after the dispute deadline, anyone can call `finalize_channel(...)`.

## Sync vs async

The headline class is the sync `NOVAIClient`. For high-throughput agents or anything already running inside an event loop, use the async `AsyncNOVAIClient` directly:

```python
from novai_sdk import AsyncNOVAIClient

async with AsyncNOVAIClient("http://localhost:3030") as client:
    nonce = await client.get_nonce(addr)
    async for signal in client.iter_signals_by_issuer(issuer, 0, 50_000):
        print(signal)
```

The two clients share the same public API. Async-native methods include `iter_*` paginators that auto-chunk past the chain's 10K-block range cap.

## Errors

Every JSON-RPC error code maps to a specific Python exception:

```python
from novai_sdk import NonceTooLowError, FeeTooLowError, MempoolFullError, ValidationError

try:
    client.transfer(alice, bob, amount=1)
except NonceTooLowError:
    # Resync and retry.
    fresh_nonce = client.get_nonce(alice.address)
    client.transfer(alice, bob, amount=1, nonce=fresh_nonce)
except FeeTooLowError:
    # Raise the fee.
    client.transfer(alice, bob, amount=1, fee=200)
```

See `novai_sdk.errors` for the full hierarchy.

## What it doesn't do (yet)

- No on-chain transaction-confirmation polling (you can implement it via `get_transaction(txid)` in a loop). Most agent workflows submit + monitor via separate paths.
- No automatic VK download for proof-submission flows. Pass `vk_bytes` directly to `build_proof_submission_extras_groth16(...)`.
- No DevnetManager: the SDK assumes a node is already reachable at the configured endpoint. Use `scripts/devnet-up.sh` (or the equivalent in the parent repo) to start a local devnet.

## Examples

See `examples/` for runnable scripts:

- `01_keygen_and_faucet.py` - generate a key, fund via faucet, check balance
- `02_register_entity.py` - register an AI entity with capabilities
- `03_post_oracle_anchor.py` - post an oracle data anchor with a tag
- `04_pay_with_splits.py` - multi-party payment using the Week 33 trailer
- `05_pay_with_condition.py` - conditional payment gated on an oracle anchor
- `06_full_lifecycle.py` - register -> discover -> SLA -> channel -> anchor -> pay

Each example expects `NOVAI_ENDPOINT` (default `http://localhost:3030`) and assumes a running devnet with the faucet enabled.

## Development

```bash
git clone https://github.com/0x-devc/NOVAI-node
cd NOVAI-node/sdk/novai-python-sdk
python3 -m venv .venv && source .venv/bin/activate
pip install -e ".[dev]"

# Run the gates:
pytest          # 278+ unit tests, all mocked, no devnet required
mypy            # strict mode, 0 errors
ruff check .    # lint clean

# Integration tests (require a running devnet at localhost:3030):
pytest -m integration
```

## License

Apache-2.0
