"""Test MarkdownV2 escaping, message formatting, and the retry path."""
from unittest import mock

from alerts import AlertSpec
from notifier import (
    MARKDOWNV2_RESERVED,
    format_alert_message,
    md_escape,
    send_alert,
)


def _spec(alert_id="block_height_stuck", playbook="EMERGENCY_FREEZE.md") -> AlertSpec:
    return AlertSpec(
        alert_id=alert_id,
        severity="CRITICAL",
        window_secs=30.0,
        summary="Block height has not advanced",
        playbook=playbook,
    )


# ---------------------------------------------------------------------------
# md_escape
# ---------------------------------------------------------------------------

def test_md_escape_covers_all_18_reserved_chars():
    # The Telegram MarkdownV2 spec reserves 18 characters; missing any one
    # produces 400 Bad Request when the message body contains it.
    expected = set("_*[]()~`>#+-=|{}.!")
    assert MARKDOWNV2_RESERVED == expected
    for ch in MARKDOWNV2_RESERVED:
        escaped = md_escape(ch)
        assert escaped == "\\" + ch, f"missed escape for {ch!r}"


def test_md_escape_passes_through_safe_text():
    assert md_escape("alpha bravo charlie") == "alpha bravo charlie"
    assert md_escape("abc123") == "abc123"
    # '=' is in MarkdownV2's 18-char reserved set per Telegram spec; escape it.
    assert md_escape("height=184231") == "height\\=184231"


def test_md_escape_escapes_period_in_filename():
    # Periods inside docs/playbooks/EMERGENCY_FREEZE.md must be escaped.
    assert md_escape("EMERGENCY_FREEZE.md") == "EMERGENCY\\_FREEZE\\.md"


def test_md_escape_handles_empty():
    assert md_escape("") == ""


# ---------------------------------------------------------------------------
# format_alert_message
# ---------------------------------------------------------------------------

def test_format_alert_message_includes_severity_alert_id_and_playbook():
    msg = format_alert_message(_spec(), "FIRE", "height=184231 (no change)", "prod-1", "2026-06-03T14:22:18Z")
    # Severity is bold (asterisks unescaped, surrounding the escaped content).
    assert msg.startswith("*CRITICAL* block\\_height\\_stuck\n")
    assert "height\\=184231 \\(no change\\)" in msg
    assert "playbook: docs/playbooks/EMERGENCY\\_FREEZE\\.md" in msg
    assert "prod\\-1 at 2026\\-06\\-03T14:22:18Z" in msg


def test_format_alert_message_omits_playbook_line_when_none():
    spec = _spec(playbook=None)
    msg = format_alert_message(spec, "FIRE", "x", "env", "2026-06-03T14:22:18Z")
    assert "playbook" not in msg


def test_format_alert_message_recover_replaces_severity_with_recovered():
    msg = format_alert_message(_spec(), "RECOVER", "height=184232", "env", "2026-06-03T14:22:18Z")
    assert msg.startswith("*RECOVERED* block\\_height\\_stuck\n")


def test_format_alert_message_no_em_dashes():
    msg = format_alert_message(_spec(), "FIRE", "height=184231", "env", "2026-06-03T14:22:18Z")
    assert "—" not in msg, "em dash leaked into formatted message"


# ---------------------------------------------------------------------------
# send_alert
# ---------------------------------------------------------------------------

def test_send_alert_dry_run_skips_network():
    # No mock needed; if it tried to hit the network with empty creds it would fail.
    assert send_alert("", "", "test", dry_run=True) is True


def test_send_alert_returns_false_without_creds_when_not_dry_run():
    assert send_alert("", "", "test", dry_run=False) is False
    assert send_alert("tok", "", "test", dry_run=False) is False
    assert send_alert("", "chat", "test", dry_run=False) is False


def test_send_alert_retries_then_gives_up_on_persistent_failure():
    sleeps = []

    def fake_sleep(d):
        sleeps.append(d)

    # Force urlopen to always raise URLError -> retries until exhaustion.
    from urllib.error import URLError
    with mock.patch("notifier.urllib.request.urlopen", side_effect=URLError("nope")):
        ok = send_alert("tok", "chat", "test", dry_run=False, sleep_fn=fake_sleep)
    assert ok is False
    # 5 attempts means 4 backoffs (no sleep after the last failure).
    assert len(sleeps) == 4
    # Exponential: 1, 2, 4, 8.
    assert sleeps == [1.0, 2.0, 4.0, 8.0]


def test_send_alert_succeeds_on_first_attempt_with_2xx():
    class FakeResp:
        status = 200
        def __enter__(self):
            return self
        def __exit__(self, *args):
            return False
        def read(self):
            return b""
    with mock.patch("notifier.urllib.request.urlopen", return_value=FakeResp()):
        assert send_alert("tok", "chat", "test", dry_run=False) is True
