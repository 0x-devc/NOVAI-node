"""PURPOSE: CoinGecko free-tier BTC/USD fetcher with retry-friendly errors.

INVARIANTS:
- Uses urllib.request only (stdlib).
- Every call honors a hard timeout (default 10s).
- Every failure path raises a CoinGeckoError subclass so the caller can
  map directly to a Prometheus reason label.

FAILURE MODES:
- 429 -> RateLimitError (with retry_after_secs).
- 5xx -> ServerError.
- urllib/OS errors / timeout -> NetworkError.
- bad JSON / missing field / non-positive -> ParseError.
"""

from __future__ import annotations

import json
import logging
import socket
import urllib.error
import urllib.request
from dataclasses import dataclass

LOG = logging.getLogger("price_oracle.coingecko")

DEFAULT_URL = (
    "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
)
DEFAULT_TIMEOUT_SECS = 10.0
USER_AGENT = "novai-price-oracle/1.0"


class CoinGeckoError(Exception):
    """Base class for CoinGecko fetch failures."""


class RateLimitError(CoinGeckoError):
    def __init__(self, retry_after_secs: float) -> None:
        super().__init__(f"rate_limited retry_after_secs={retry_after_secs:.0f}")
        self.retry_after_secs = retry_after_secs


class ServerError(CoinGeckoError):
    def __init__(self, status: int) -> None:
        super().__init__(f"server_error status={status}")
        self.status = status


class NetworkError(CoinGeckoError):
    pass


class ParseError(CoinGeckoError):
    pass


@dataclass(frozen=True)
class PriceObservation:
    coin: str
    fiat: str
    price: float


def fetch_btc_usd(
    url: str = DEFAULT_URL,
    timeout: float = DEFAULT_TIMEOUT_SECS,
    opener: urllib.request.OpenerDirector | None = None,
) -> PriceObservation:
    """Fetch the current BTC/USD spot from CoinGecko.

    ``opener`` is injectable so tests can swap the network layer without
    monkey-patching the global urllib state.
    """
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    open_fn = opener.open if opener is not None else urllib.request.urlopen
    try:
        with open_fn(req, timeout=timeout) as resp:
            status = getattr(resp, "status", 200)
            body = resp.read()
    except urllib.error.HTTPError as exc:
        if exc.code == 429:
            raise RateLimitError(_parse_retry_after(exc.headers.get("Retry-After"))) from exc
        if 500 <= exc.code <= 599:
            raise ServerError(exc.code) from exc
        raise NetworkError(f"http_status status={exc.code}") from exc
    except (urllib.error.URLError, socket.timeout, TimeoutError, OSError) as exc:
        raise NetworkError(f"network_error error={exc}") from exc

    if status != 200:
        raise NetworkError(f"unexpected_status status={status}")
    return _parse_btc_usd(body)


def _parse_btc_usd(body: bytes) -> PriceObservation:
    try:
        data = json.loads(body)
    except json.JSONDecodeError as exc:
        raise ParseError(f"json_decode error={exc}") from exc
    try:
        price = float(data["bitcoin"]["usd"])
    except (KeyError, TypeError, ValueError) as exc:
        raise ParseError(f"missing_field error={exc}") from exc
    if price <= 0:
        raise ParseError(f"non_positive_price price={price}")
    return PriceObservation(coin="bitcoin", fiat="usd", price=price)


def _parse_retry_after(header_value: str | None) -> float:
    if not header_value:
        return 60.0
    try:
        return max(0.0, float(header_value))
    except (TypeError, ValueError):
        return 60.0


class BackoffState:
    """Exponential backoff ladder for rate-limited fetches.

    Ladder is 60, 120, 240, 300 (then held at 300). Reset on the next
    successful fetch. The instance is not thread-safe; the oracle loop is
    single-threaded for chain interaction.
    """

    LADDER: tuple[float, ...] = (60.0, 120.0, 240.0, 300.0)

    def __init__(self) -> None:
        self._index = 0

    def on_rate_limit(self) -> float:
        delay = self.LADDER[min(self._index, len(self.LADDER) - 1)]
        self._index = min(self._index + 1, len(self.LADDER) - 1)
        return delay

    def reset(self) -> None:
        self._index = 0

    @property
    def index(self) -> int:
        return self._index
