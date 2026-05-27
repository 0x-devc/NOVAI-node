# Changelog

All notable changes to the NOVAI Python SDK.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-05-26

Initial release. Wraps NOVAI testnet-v0.1 (chain Weeks 1-36).

### Added

#### Transactions (11 types)

- `build_transfer_payload` (type 1)
- `build_signal_commitment_payload` (type 2, envelope only)
- `build_create_memory_payload` / `build_update_memory_payload` /
  `build_delete_memory_payload` (types 3, 4, 5)
- `build_submit_proposal_payload` / `build_execute_proposal_payload`
  (types 6, 7)
- `build_register_entity_payload` (type 8, 51 bytes fixed)
- `build_credit_entity_payload` (type 9, 49 bytes fixed)
- `build_register_with_key_payload` (type 10, 83 bytes fixed)
- `build_entity_upgrade_payload` (type 11, 97 bytes fixed, Week 34)

#### Signal extras (23 types)

- `build_empty_extras` for types 0-6 (Anomaly, Optimization, Prediction,
  RiskScore, AuditReport, SpamRisk, CongestionForecast)
- `build_reputation_update_extras` (type 7)
- `build_signal_purchase_extras` (type 8)
- `build_stake_deposit_extras` / `_withdraw_` / `_slash_` (types 9-11)
- `build_composition_check_extras` (type 12)
- `build_proof_submission_extras_v1_stub` / `_groth16` / `_groth16_registered`
  (type 13, with Week 30 registered-VK variant)
- `build_subscription_create_extras` / `_cancel_` (types 14, 15)
- `build_payment_request_extras` with optional Week 33 `splits` and
  Week 36 `condition` kwargs (type 16)
- `build_service_attestation_extras` (type 17)
- `build_sla_accept_extras` + `derive_sla_accept_signal_hash` (type 18,
  Week 31)
- `build_channel_accept_extras` / `_close_` / `_finalize_extras`
  (types 19-21, Week 32)
- `build_oracle_anchor_extras` + `derive_oracle_anchor_signal_hash`
  (type 22, Week 35)

#### Memory object data block encoders

- `encode_service_descriptor` (type 12, 144 bytes fixed, Week 29)
- `encode_vk_registration` (type 13, variable, Week 30)
- `encode_sla_agreement` (type 14, 210 bytes fixed, Week 31)
- `encode_payment_channel` (type 15, 222 bytes fixed, Week 32)

#### High-level client API

- `AsyncNOVAIClient` and `NOVAIClient` (sync facade)
- Read RPCs: `get_transaction`, `get_block_by_height` / `_by_hash` /
  `get_latest_block`, `get_balance`, `get_nonce`, `get_ai_entity`,
  `get_upgrade_history`, `get_memory_objects`,
  `get_signals_by_height` / `_by_issuer` / `_by_type`,
  `get_payments_by_entity`, `discover_services`
  (alias of `get_service_descriptors_by_category`),
  `get_vk_registration`, `list_vk_registrations`,
  `get_sla_agreement`, `get_active_sla`,
  `list_slas_by_buyer` / `_by_seller`,
  `get_payment_channel`, `list_channels_by_party_a` / `_b`,
  `get_channel_dispute_status`,
  `get_oracle_anchors_by_entity` / `_by_tag` (with optional ts_min /
  ts_max), `get_oracle_anchor`
- Write convenience methods (build + sign + submit + typed result):
  `transfer`, `register_entity`, `register_entity_with_key`,
  `credit_entity`, `upgrade_entity`, `create_memory_object` /
  `update_memory_object` / `delete_memory_object`, `publish_signal`,
  `pay` (with optional splits + condition), `attest_payment`,
  `post_oracle_anchor` (auto-derives signal hash), `accept_sla`,
  `accept_channel`, `close_channel`, `finalize_channel`
- Auto-paginators: `iter_signals_by_issuer`, `iter_payments_by_entity`
  (chunk past the chain's 10K-block range cap)
- Faucet helper: `faucet(address)`

#### Cryptography

- `Keypair` dataclass with `generate` / `from_seed` / `load` / `save`
  / `sign` (raw 32-byte seed file format, CLI-compatible)
- `address_from_pubkey` (blake3 with `"NOVAI_ADDRESS_V1"` domain)
- `compute_entity_id` (blake3 with `"NOVAI_AI_ENTITY_ID_V1"` domain)
- `sign_tx_v1` / `verify_tx_v1` (ed25519 over `b"NOVAI_TX_V1" ||
  encode_tx_v1_unsigned(tx)`)
- `sign_channel_state` / `verify_channel_state_signature` (BE-encoded
  167-byte canonical channel state with `"NOVAI_CHANNEL_STATE_V1"`
  domain; off-chain Week 32 helper)
- `blake3_hash` / `blake3_keyed` convenience wrappers

#### Codec

- `TxV1` dataclass + `encode_tx_v1_unsigned` / `_signed` / `txid_v1` /
  `tx_encoded_size` / `decode_tx_v1_signed` (LE envelope, BE payload
  internals)

#### Typed response dataclasses (in `novai_sdk.types`)

- `BlockHeader`, `TxReceipt`, `AiEntityInfo`, `UpgradeRecord`,
  `MemoryObjectInfo`, `SignalInfo`, `PaymentRecord` +
  `PaymentSplitJson` + `PaymentConditionJson`,
  `ServiceDescriptorInfo`, `VkRegistrationInfo`, `SlaAgreementInfo`,
  `PaymentChannelInfo`, `ChannelDisputeStatus`, `OracleAnchorInfo`,
  `SubmissionResult`

#### Error hierarchy

- `NovaiError` base; `NovaiRpcError` for JSON-RPC errors;
  specific subclasses for every documented error code
  (`NonceTooLowError` -> -32010, `FeeTooLowError` -> -32011,
  `MempoolFullError` -> -32001, `SenderLimitExceededError` -> -32012,
  `ValidationError` -> -32013, `StateQueryError` -> -32002,
  `ResponseTooLargeError` -> -32003, `MethodNotFoundError`,
  `InvalidParamsError`, `ParseError`, `InvalidRequestError`,
  `ServerError`, `RateLimitedError` for faucet cooldowns,
  `EncodingError`, `DecodeError`)

### Notes

- Python 3.9+ supported; mypy strict mode passes on Python 3.10+
- Pure Python wheel (no compiled extensions); PyNaCl provides the
  ed25519 backend via libsodium wheels
- 278 unit tests + 7 integration smoke tests (require a running devnet)
- Byte-level parity with the Rust codec verified across 11 tx types,
  23 signal types, 4 memory object types, and the off-chain channel
  state signing flow
- TS SDK signing audit follow-up tracked at
  `TODO_TS_SDK_SIGNING_AUDIT.md` (repo root)
- PaymentRequest trailer dispatch design debt documented at
  `docs/PAYMENT_REQUEST_TRAILER_VERSIONING.md`
