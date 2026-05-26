# NOVAI Python SDK

Pure-Python client for the [NOVAI](https://github.com/0x-devc/NOVAI-node) blockchain.

`novai-sdk` lets agent-framework code (LangChain, CrewAI, AutoGen, custom Python tools) talk to a NOVAI node without any Rust toolchain. It wraps the full JSON-RPC surface, signs transactions locally with ed25519 (via PyNaCl), and constructs every payload byte-for-byte the way the Rust node expects.

> Phase 1 status: keypair management, TxV1 envelope codec, signing primitives, and a minimal async/sync RPC client (`submit_tx`, `get_nonce`, `get_balance`, `faucet`) are shipped. Phase 2 fills in builders for the 11 tx types and 23 signal types. Phase 3 adds the typed high-level client. Phase 4 ships integration tests and the full README. Phase 5 publishes to PyPI.

## Install (development, from the monorepo)

```bash
cd sdk/novai-python-sdk
python3 -m venv .venv && source .venv/bin/activate
pip install -e ".[dev]"
```

## Quick example (Phase 1 surface)

```python
from novai_sdk import NOVAIClient, Keypair

client = NOVAIClient("http://localhost:3030")
kp = Keypair.generate()

# Drop some test funds (requires the node to be running with --dev-keys or --faucet-key)
result = client.faucet(kp.address)
print(f"faucet txid: {result.txid}, amount: {result.amount}")

# Check the balance
balance = client.get_balance(kp.address)
print(f"balance: {balance.balance}, nonce: {balance.nonce}")
```

## Compatibility

- Python >= 3.9
- Wire format pinned to NOVAI testnet-v0.1 (Weeks 1-36)
- Key file format: raw 32-byte ed25519 seed (compatible with `novai-cli keygen`)

## Layout

```
novai_sdk/
├── client.py        # AsyncNOVAIClient (aiohttp transport)
├── sync_client.py   # NOVAIClient (sync wrapper via asyncio.run)
├── keys.py          # Keypair: generate / from_seed / load / save / sign
├── crypto.py        # sign_tx_v1, sign_channel_state, address_from_pubkey
├── codec.py         # TxV1 + canonical encode/decode (matches crates/codec)
├── capabilities.py  # Capabilities bitmask (read_only / advisory / gated / oracle)
├── constants.py     # PAYMENT_CONDITION_MARKER, BPS_DENOMINATOR, domain separators
├── enums.py         # AiSignalType, MemoryObjectType, AutonomyMode, ProofType, ...
├── errors.py        # NonceTooLowError, FeeTooLowError, ValidationError, ...
├── tx/              # (Phase 2) per-tx-type payload builders
├── signals/         # (Phase 2) per-signal-type extras encoders
└── memory_objects/  # (Phase 2) per-memory-object-type builders
```

## License

Apache-2.0
