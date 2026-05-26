"""Protocol constants pinned to the Rust implementation.

Every constant here mirrors a value defined in ``crates/`` under the matching
name. If the Rust value moves, this file moves with it. The SDK treats these
as immutable wire-format anchors; touching one is a hard fork.
"""

from __future__ import annotations

# Domain separators (see crates/crypto/src/lib.rs and crates/ai_entities/src/lib.rs).
DOMAIN_TAG_TX_V1: bytes = b"NOVAI_TX_V1"
DOMAIN_TAG_ADDRESS_V1: bytes = b"NOVAI_ADDRESS_V1"
DOMAIN_TAG_AI_ENTITY_ID_V1: bytes = b"NOVAI_AI_ENTITY_ID_V1"
DOMAIN_TAG_CHANNEL_STATE_V1: bytes = b"NOVAI_CHANNEL_STATE_V1"
DOMAIN_TAG_ORACLE_ANCHOR_TAG_V1: bytes = b"NOVAI_ORACLE_ANCHOR_TAG_V1"

# TxV1 envelope (see crates/codec/src/lib.rs).
TX_V1_VERSION: int = 1
# 1(version) + 32(from) + 32(pubkey) + 8(nonce LE) + 8(fee LE) + 4(payload_len LE) + 64(sig)
TX_V1_OVERHEAD: int = 149
# crates/types: MAX_TX_SIZE.
MAX_TX_SIZE: int = 131_072  # 128 KB

# Memory object limits (see crates/ai_entities/src/memory.rs).
MAX_MEMORY_OBJECT_SIZE: int = 65_536  # 64 KB
MAX_MEMORY_OBJECTS_PER_ENTITY: int = 100

# Payment fee + splits (see crates/execution/src/lib.rs).
BPS_DENOMINATOR: int = 10_000
PAYMENT_FEE_BPS: int = 200  # 2 percent marketplace fee
MARKETPLACE_FEE_BPS: int = 200
SUBSCRIPTION_CANCEL_FEE_BPS: int = 500
MAX_PAYMENT_SPLITS: int = 8
MIN_PAYMENT_SPLITS_WHEN_PRESENT: int = 2
PAYMENT_SPLIT_SIZE: int = 34  # 32 byte recipient + 2 byte u16 BE basis_points

# Payment condition (Week 36; see crates/execution/src/lib.rs:1357).
PAYMENT_CONDITION_MARKER: int = 0xC1
# Offset inside the PaymentRequest tail where the splits-count byte (or the
# 0xC1 condition marker) lives. The legacy 178-byte payload ends exactly here.
PAYMENT_CONDITION_DISPATCH_OFFSET: int = 178

# Oracle anchor (Week 35; see crates/execution/src/lib.rs:1467-1500).
ORACLE_ANCHOR_DATA_TAG_MAX_LEN: int = 32
ORACLE_ANCHOR_DATA_TAG_MIN_LEN: int = 1
# data_hash(32) + external_timestamp(8) + source_hash(32) + expiry_height(8) + tag_len(1)
ORACLE_ANCHOR_EXTRA_FIXED_LEN: int = 81
ORACLE_ANCHOR_EXTRA_MIN_LEN: int = ORACLE_ANCHOR_EXTRA_FIXED_LEN + 1  # 82
ORACLE_ANCHOR_EXTRA_MAX_LEN: int = ORACLE_ANCHOR_EXTRA_FIXED_LEN + ORACLE_ANCHOR_DATA_TAG_MAX_LEN

# Entity upgrade cooldown (Week 34).
MIN_UPGRADE_INTERVAL_BLOCKS: int = 1_000

# Payment channel chain ID (Week 32).
# Pinned to the on-chain constant used by `sign_channel_state` so that an
# off-chain signature is bound to NOVAI and cannot be replayed on a fork.
NOVAI_CHANNEL_CHAIN_ID: int = 1

# Per-entity caps on memory object types (see crates/ai_entities/src/memory.rs).
MAX_DELEGATION_GRANTS: int = 20
MAX_SUBSCRIPTIONS_PER_ENTITY: int = 10
MAX_SERVICE_DESCRIPTORS_PER_ENTITY: int = 16
MAX_VK_REGISTRATIONS_PER_ENTITY: int = 8
MAX_SLAS_PER_ENTITY: int = 8

# Proof submission caps (Week 30).
PROOF_SUBMISSION_MAX_VK_BYTES: int = 8_192
PROOF_SUBMISSION_MAX_PROOF_BYTES: int = 1_024

# Subscription duration floor (Week 28+).
MIN_SUBSCRIPTION_DURATION: int = 100

# RPC range cap on signal/payment/upgrade/channel/oracle queries (Phase 0 finding).
MAX_QUERY_HEIGHT_RANGE: int = 10_000
