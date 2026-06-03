"""
PURPOSE: Format and deliver alert notifications to a Telegram bot DM.
Plain stdlib HTTP, no third-party dependencies. Supports a dry-run mode
that logs the rendered message instead of sending so smoke tests do not
spam the operator's phone.

INVARIANTS:
- format_alert_message is a pure function over its inputs. send_alert is
  the only function that performs network I/O.
- All content text is escaped per Telegram MarkdownV2 rules before being
  wrapped in formatting markup. The asterisks used for bold severity are
  the only un-escaped MarkdownV2 syntax in the message.
- send_alert never raises on network failure. It returns False after the
  retry budget is exhausted and the caller is responsible for handling
  the durable buffer.

FAILURE MODES:
- Telegram API rejects malformed MarkdownV2 with HTTP 400. If the escape
  helper misses a character the message will not arrive. The escape set
  is exhaustive per Telegram's published spec (18 reserved chars).
- Rate limits (~1 msg/sec/chat) are nowhere near our realistic alert
  volume; no token bucket is implemented.
"""

from __future__ import annotations

import json
import logging
import time
import urllib.error
import urllib.request
from typing import Optional

from alerts import AlertSpec

log = logging.getLogger("novai_monitor.notifier")

TELEGRAM_API_BASE = "https://api.telegram.org"

# https://core.telegram.org/bots/api#markdownv2-style
MARKDOWNV2_RESERVED = set("_*[]()~`>#+-=|{}.!")


def md_escape(text: str) -> str:
    """Escape every Telegram MarkdownV2 reserved character with a backslash."""
    out = []
    for ch in text:
        if ch in MARKDOWNV2_RESERVED:
            out.append("\\")
        out.append(ch)
    return "".join(out)


def format_alert_message(
    spec: AlertSpec,
    transition: str,
    detail: str,
    env_label: str,
    now_iso: str,
) -> str:
    """
    Build a Telegram MarkdownV2 message body for a FIRE or RECOVER transition.
    Lines: severity + alert_id, detail, threshold window, location, optional playbook.
    """
    severity = spec.severity if transition == "FIRE" else "RECOVERED"
    head = f"*{md_escape(severity)}* {md_escape(spec.alert_id)}"
    body = [
        head,
        md_escape(detail),
        md_escape(f"threshold: {spec.window_secs:.0f}s window, {spec.summary}"),
        md_escape(f"{env_label} at {now_iso}"),
    ]
    if spec.playbook:
        body.append(md_escape(f"playbook: docs/playbooks/{spec.playbook}"))
    return "\n".join(body)


def send_alert(
    bot_token: str,
    chat_id: str,
    message: str,
    dry_run: bool = False,
    max_attempts: int = 5,
    base_backoff_secs: float = 1.0,
    max_backoff_secs: float = 30.0,
    request_timeout_secs: float = 10.0,
    sleep_fn=time.sleep,
) -> bool:
    """
    POST a MarkdownV2-formatted message to a Telegram chat with exponential
    backoff retry. Returns True on delivery, False after the retry budget
    is exhausted. Never raises.
    """
    if dry_run:
        log.info("notifier_dry_run event=would_send message=%r", message)
        return True
    if not bot_token or not chat_id:
        log.error("notifier_missing_creds event=cannot_send token_set=%s chat_set=%s",
                  bool(bot_token), bool(chat_id))
        return False
    url = f"{TELEGRAM_API_BASE}/bot{bot_token}/sendMessage"
    payload = json.dumps({
        "chat_id": chat_id,
        "text": message,
        "parse_mode": "MarkdownV2",
        "disable_web_page_preview": True,
    }).encode("utf-8")
    for attempt in range(1, max_attempts + 1):
        try:
            req = urllib.request.Request(
                url,
                data=payload,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=request_timeout_secs) as resp:
                if 200 <= resp.status < 300:
                    log.info("notifier_sent event=ok attempt=%d status=%d", attempt, resp.status)
                    return True
                log.warning("notifier_non_2xx event=retry attempt=%d status=%d",
                            attempt, resp.status)
        except urllib.error.HTTPError as e:
            log.warning("notifier_http_error event=retry attempt=%d status=%d reason=%s",
                        attempt, e.code, e.reason)
        except (urllib.error.URLError, OSError) as e:
            log.warning("notifier_network_error event=retry attempt=%d error=%s",
                        attempt, e)
        if attempt < max_attempts:
            backoff = min(base_backoff_secs * (2 ** (attempt - 1)), max_backoff_secs)
            sleep_fn(backoff)
    log.error("notifier_exhausted event=give_up attempts=%d", max_attempts)
    return False


def render_for_stderr(spec: AlertSpec, transition: str, detail: str, env_label: str, now_iso: str) -> str:
    """Plain-text rendering of an alert for stderr/journald, no MarkdownV2 escaping."""
    severity = spec.severity if transition == "FIRE" else "RECOVERED"
    parts = [
        f"event=alert_{transition.lower()}",
        f"alert_id={spec.alert_id}",
        f"severity={severity}",
        f"detail={detail!r}",
        f"window_secs={spec.window_secs:.0f}",
        f"env={env_label}",
        f"ts={now_iso}",
    ]
    if spec.playbook:
        parts.append(f"playbook=docs/playbooks/{spec.playbook}")
    return " ".join(parts)


def append_undelivered(path: str, spec: AlertSpec, transition: str, message: str, now_iso: str) -> Optional[Exception]:
    """
    Append an undelivered alert to a JSONL buffer so it can be replayed on the
    next successful send. Returns the exception on failure (caller logs),
    or None on success.
    """
    record = {
        "ts": now_iso,
        "alert_id": spec.alert_id,
        "severity": spec.severity,
        "transition": transition,
        "message": message,
    }
    try:
        with open(path, "a", encoding="utf-8") as f:
            f.write(json.dumps(record) + "\n")
        return None
    except OSError as e:
        return e
