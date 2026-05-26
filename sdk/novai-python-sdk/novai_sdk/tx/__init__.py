"""Transaction payload builders.

Each module here builds the binary payload for one tx type (1..=11). The
builders return raw bytes; the caller wraps them in a :class:`TxV1` envelope
via :func:`novai_sdk.codec.encode_tx_v1_signed`. Phase 2 fills in the
individual builders; Phase 1 leaves this package empty as a placeholder.
"""
