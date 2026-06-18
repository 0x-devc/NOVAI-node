"""PURPOSE: Public GPU pricing fetcher with retry-friendly error classes.

The agent observes GPU rental pricing from a public marketplace bundles API
(Vast.ai by default). This is a read-only HTTP GET against a public pricing
source; it is NOT a chain RPC. The response is expected to carry a list of
offers, each with a GPU model name, a total dollars-per-hour figure, and a
GPU count. The observation is the MEDIAN per-GPU on-demand USD-per-hour for
the configured model across currently-listed offers.

INVARIANTS:
- Uses urllib.request only (stdlib).
- Every call honors a hard timeout (default 10s).
- Every failure path raises a GpuSourceError subclass so the caller can map
  directly to a Prometheus reason label.
- When the source responds fine but carries no usable offer for the model,
  NoDataError is raised. The loop skips the cycle. The agent never posts a
  fabricated or stale price.

FAILURE MODES:
- 429 -> RateLimitError (with retry_after_secs).
- 5xx -> ServerError.
- urllib/OS errors / timeout -> NetworkError.
- bad JSON / wrong top-level shape -> ParseError.
- valid response, no offer for the model -> NoDataError.
"""

from __future__ import annotations

import json
import logging
import socket
import statistics
import urllib.error
import urllib.request
from dataclasses import dataclass

LOG = logging.getLogger("compute_oracle.gpu_source")

DEFAULT_URL = "https://console.vast.ai/api/v0/bundles/"
DEFAULT_TIMEOUT_SECS = 10.0
DEFAULT_MODEL = "RTX 4090"
DEFAULT_SOURCE_ID = "vast.ai/api/v0/bundles"
USER_AGENT = "novai-compute-oracle/1.0"


class GpuSourceError(Exception):
    """Base class for GPU pricing fetch failures."""


class RateLimitError(GpuSourceError):
    def __init__(self, retry_after_secs: float) -> None:
        super().__init__(f"rate_limited retry_after_secs={retry_after_secs:.0f}")
        self.retry_after_secs = retry_after_secs


class ServerError(GpuSourceError):
    def __init__(self, status: int) -> None:
        super().__init__(f"server_error status={status}")
        self.status = status


class NetworkError(GpuSourceError):
    pass


class ParseError(GpuSourceError):
    pass


class NoDataError(GpuSourceError):
    """Source responded but carried no usable offer for the model."""


@dataclass(frozen=True)
class GpuPriceObservation:
    model: str
    currency: str
    unit: str
    price: float
    sample_size: int
    source: str


def normalize_model_label(model: str) -> str:
    """Canonical label for a GPU model: uppercase, no spaces.

    "RTX 4090" -> "RTX4090". Used for the observation encoding and to match
    free-form ``gpu_name`` strings returned by the marketplace.
    """
    return "".join(model.split()).upper()


def fetch_gpu_price(
    url: str = DEFAULT_URL,
    timeout: float = DEFAULT_TIMEOUT_SECS,
    model: str = DEFAULT_MODEL,
    opener: urllib.request.OpenerDirector | None = None,
    source_id: str = DEFAULT_SOURCE_ID,
) -> GpuPriceObservation:
    """Fetch current GPU pricing and reduce it to a single observation.

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
    return _parse_offers(body, model=model, source_id=source_id)


def _parse_offers(body: bytes, *, model: str, source_id: str) -> GpuPriceObservation:
    try:
        data = json.loads(body)
    except json.JSONDecodeError as exc:
        raise ParseError(f"json_decode error={exc}") from exc
    if not isinstance(data, dict):
        raise ParseError(f"unexpected_top_level type={type(data).__name__}")
    offers = data.get("offers")
    if not isinstance(offers, list):
        raise ParseError("missing_or_malformed_offers")

    match_key = normalize_model_label(model).lower()
    per_gpu_prices: list[float] = []
    for offer in offers:
        if not isinstance(offer, dict):
            continue
        gpu_name = offer.get("gpu_name")
        if not isinstance(gpu_name, str):
            continue
        if match_key not in normalize_model_label(gpu_name).lower():
            continue
        try:
            dph_total = float(offer["dph_total"])
            num_gpus = int(offer.get("num_gpus", 1))
        except (KeyError, TypeError, ValueError):
            continue
        if num_gpus < 1 or dph_total <= 0:
            continue
        per_gpu = dph_total / num_gpus
        if per_gpu > 0:
            per_gpu_prices.append(per_gpu)

    if not per_gpu_prices:
        raise NoDataError(f"no_offers_for_model model={model}")

    median_price = float(statistics.median(per_gpu_prices))
    return GpuPriceObservation(
        model=normalize_model_label(model),
        currency="usd",
        unit="hour",
        price=median_price,
        sample_size=len(per_gpu_prices),
        source=source_id,
    )


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
