"""Tests for novai_sdk.errors (RPC error mapping)."""

from __future__ import annotations

import pytest

from novai_sdk.errors import (
    FeeTooLowError,
    InvalidParamsError,
    MempoolFullError,
    MethodNotFoundError,
    NonceTooLowError,
    NovaiRpcError,
    ParseError,
    RateLimitedError,
    ResponseTooLargeError,
    SenderLimitExceededError,
    ServerError,
    StateQueryError,
    ValidationError,
    rpc_error_from,
)


@pytest.mark.parametrize(
    "code,cls",
    [
        (-32700, ParseError),
        (-32601, MethodNotFoundError),
        (-32602, InvalidParamsError),
        (-32000, ServerError),
        (-32001, MempoolFullError),
        (-32002, StateQueryError),
        (-32003, ResponseTooLargeError),
        (-32010, NonceTooLowError),
        (-32011, FeeTooLowError),
        (-32012, SenderLimitExceededError),
        (-32013, ValidationError),
    ],
)
def test_code_maps_to_specific_class(code: int, cls: type[NovaiRpcError]) -> None:
    err = rpc_error_from(code, "test message")
    assert isinstance(err, cls)
    assert err.code == code
    assert err.message == "test message"


def test_unknown_code_falls_back_to_base() -> None:
    err = rpc_error_from(-99999, "unknown")
    assert type(err) is NovaiRpcError


def test_faucet_rate_limit_promoted() -> None:
    """The chain returns -32000 for faucet rate-limits; the SDK promotes them."""
    err = rpc_error_from(-32000, "faucet rate limited, retry in 60s")
    assert isinstance(err, RateLimitedError)


def test_generic_server_error_is_not_rate_limited() -> None:
    err = rpc_error_from(-32000, "internal database error")
    assert isinstance(err, ServerError)
    assert not isinstance(err, RateLimitedError)


def test_data_field_preserved() -> None:
    err = rpc_error_from(-32602, "bad param", data={"field": "address"})
    assert err.data == {"field": "address"}


def test_error_inherits_from_novai_rpc_error() -> None:
    err = rpc_error_from(-32010, "nonce too low")
    assert isinstance(err, NovaiRpcError)
    # And it can be raised + caught.
    with pytest.raises(NovaiRpcError):
        raise err
    with pytest.raises(NonceTooLowError):
        raise err
