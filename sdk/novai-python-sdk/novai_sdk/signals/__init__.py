"""Per-signal-type extras encoders.

Each function returns the variable-length extras tail that follows the
66-byte SignalCommitment envelope. Wrap the envelope and extras via
:func:`novai_sdk.tx.build_signal_commitment_payload`.
"""

from novai_sdk.signals.basic import build_empty_extras
from novai_sdk.signals.channels import (
    build_channel_accept_extras,
    build_channel_close_extras,
    build_channel_finalize_extras,
)
from novai_sdk.signals.composition import build_composition_check_extras
from novai_sdk.signals.oracle import (
    build_oracle_anchor_extras,
    derive_oracle_anchor_signal_hash,
)
from novai_sdk.signals.payments import (
    PaymentCondition,
    PaymentSplit,
    build_payment_request_extras,
    build_service_attestation_extras,
    validate_splits,
)
from novai_sdk.signals.proof import (
    build_proof_submission_extras_groth16,
    build_proof_submission_extras_groth16_registered,
    build_proof_submission_extras_v1_stub,
)
from novai_sdk.signals.purchase import build_signal_purchase_extras
from novai_sdk.signals.reputation import build_reputation_update_extras
from novai_sdk.signals.sla import (
    build_sla_accept_extras,
    derive_sla_accept_signal_hash,
)
from novai_sdk.signals.stake import (
    build_stake_deposit_extras,
    build_stake_slash_extras,
    build_stake_withdraw_extras,
)
from novai_sdk.signals.subscription import (
    build_subscription_cancel_extras,
    build_subscription_create_extras,
)

__all__ = [
    "PaymentCondition",
    "PaymentSplit",
    "build_channel_accept_extras",
    "build_channel_close_extras",
    "build_channel_finalize_extras",
    "build_composition_check_extras",
    "build_empty_extras",
    "build_oracle_anchor_extras",
    "build_payment_request_extras",
    "build_proof_submission_extras_groth16",
    "build_proof_submission_extras_groth16_registered",
    "build_proof_submission_extras_v1_stub",
    "build_reputation_update_extras",
    "build_service_attestation_extras",
    "build_signal_purchase_extras",
    "build_sla_accept_extras",
    "build_stake_deposit_extras",
    "build_stake_slash_extras",
    "build_stake_withdraw_extras",
    "build_subscription_cancel_extras",
    "build_subscription_create_extras",
    "derive_oracle_anchor_signal_hash",
    "derive_sla_accept_signal_hash",
    "validate_splits",
]
