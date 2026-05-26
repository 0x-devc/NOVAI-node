"""Exception classes for the NOVAI SDK.

The chain returns precise JSON-RPC error codes; this module maps each one to a
dedicated Python exception class so that callers can ``except NonceTooLowError``
and recover programmatically rather than parsing error strings.
"""

from __future__ import annotations

from typing import Any


class NovaiError(Exception):
    """Base class for every error raised by the SDK."""


class EncodingError(NovaiError):
    """Raised when a payload cannot be built (invalid input length, range, etc.)."""


class DecodeError(NovaiError):
    """Raised when an RPC response cannot be parsed."""


class NovaiRpcError(NovaiError):
    """Base class for errors returned by the node over JSON-RPC."""

    def __init__(self, code: int, message: str, data: Any | None = None) -> None:
        self.code = code
        self.message = message
        self.data = data
        super().__init__(f"RPC error {code}: {message}")


class ParseError(NovaiRpcError):
    """JSON-RPC -32700: server could not parse the request body."""


class InvalidRequestError(NovaiRpcError):
    """JSON-RPC -32600: request was not a valid JSON-RPC envelope."""


class MethodNotFoundError(NovaiRpcError):
    """JSON-RPC -32601: the method does not exist on this node."""


class InvalidParamsError(NovaiRpcError):
    """JSON-RPC -32602: a parameter was missing, malformed, or out of range."""


class ServerError(NovaiRpcError):
    """JSON-RPC -32000: generic server-side error (bad tx encoding, oversized payload, etc.)."""


class MempoolFullError(NovaiRpcError):
    """JSON-RPC -32001: mempool is at capacity or rejected the tx."""


class StateQueryError(NovaiRpcError):
    """JSON-RPC -32002: state read failed (database error, missing row, etc.)."""


class ResponseTooLargeError(NovaiRpcError):
    """JSON-RPC -32003: serialized response would exceed 10 MB."""


class NonceTooLowError(NovaiRpcError):
    """JSON-RPC -32010: submitted tx has a nonce below the account's next expected value.

    Recovery: call :meth:`client.get_nonce` and resubmit with the fresh value.
    """


class FeeTooLowError(NovaiRpcError):
    """JSON-RPC -32011: submitted tx fee is below the minimum for this payload type.

    Recovery: raise the fee and resubmit. Default per-command fees live in
    ``novai_sdk.tx`` builders.
    """


class SenderLimitExceededError(NovaiRpcError):
    """JSON-RPC -32012: too many pending txs from this sender in the mempool."""


class ValidationError(NovaiRpcError):
    """JSON-RPC -32013: tx failed deterministic validation on the chain side."""


class RateLimitedError(NovaiRpcError):
    """Faucet or other rate-limited endpoint rejected the call.

    The chain returns ``-32000`` with a message describing the remaining
    cooldown; the SDK promotes faucet rate-limit errors to this class so
    callers can distinguish them from other ``-32000`` failures.
    """


_CODE_TO_CLASS: dict[int, type[NovaiRpcError]] = {
    -32700: ParseError,
    -32600: InvalidRequestError,
    -32601: MethodNotFoundError,
    -32602: InvalidParamsError,
    -32000: ServerError,
    -32001: MempoolFullError,
    -32002: StateQueryError,
    -32003: ResponseTooLargeError,
    -32010: NonceTooLowError,
    -32011: FeeTooLowError,
    -32012: SenderLimitExceededError,
    -32013: ValidationError,
}


def rpc_error_from(code: int, message: str, data: Any | None = None) -> NovaiRpcError:
    """Construct the most specific RPC error class for ``code``.

    Falls back to the base :class:`NovaiRpcError` for unknown codes. Faucet
    rate-limit failures (``-32000`` with ``rate limit`` in the message) are
    promoted to :class:`RateLimitedError`.
    """
    if code == -32000 and "rate" in message.lower():
        return RateLimitedError(code, message, data)
    cls = _CODE_TO_CLASS.get(code, NovaiRpcError)
    return cls(code, message, data)
