"""
Tests for the JSON-RPC client and the metrics fetch error handling. Network I/O
is monkeypatched at urllib.request.urlopen, the same approach used in
test_notifier.py.
"""
import json

import novai_monitor as nm


class _FakeResp:
    def __init__(self, status, body):
        self.status = status
        self._body = body

    def read(self):
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, *_a):
        return False


def _patch_urlopen(monkeypatch, resp_or_exc):
    def _urlopen(_req, timeout=0):
        if isinstance(resp_or_exc, Exception):
            raise resp_or_exc
        return resp_or_exc
    monkeypatch.setattr(nm.urllib.request, "urlopen", _urlopen)


def test_rpc_call_parses_success(monkeypatch):
    body = json.dumps(
        {"jsonrpc": "2.0", "result": {"height": 184231, "state_root": "0xabc123"}, "id": 1}
    ).encode("utf-8")
    _patch_urlopen(monkeypatch, _FakeResp(200, body))
    result, err = nm.rpc_call("http://localhost:3030", "novai_getLatestBlock", None, 5.0)
    assert err is None
    assert result["height"] == 184231
    assert result["state_root"] == "0xabc123"


def test_rpc_call_handles_error_envelope(monkeypatch):
    body = json.dumps(
        {"jsonrpc": "2.0", "error": {"code": -32601, "message": "method not found"}, "id": 1}
    ).encode("utf-8")
    _patch_urlopen(monkeypatch, _FakeResp(200, body))
    result, err = nm.rpc_call("http://localhost:3030", "novai_getLatestBlock", None, 5.0)
    assert result is None
    assert "rpc_error" in err


def test_rpc_call_handles_null_result(monkeypatch):
    body = json.dumps({"jsonrpc": "2.0", "result": None, "id": 1}).encode("utf-8")
    _patch_urlopen(monkeypatch, _FakeResp(200, body))
    result, err = nm.rpc_call("http://localhost:3030", "novai_getLatestBlock", None, 5.0)
    assert result is None
    assert err == "no_result"


def test_rpc_call_handles_bad_json(monkeypatch):
    _patch_urlopen(monkeypatch, _FakeResp(200, b"not json at all"))
    result, err = nm.rpc_call("http://localhost:3030", "novai_getLatestBlock", None, 5.0)
    assert result is None
    assert err == "bad_json"


def test_rpc_call_handles_unreachable(monkeypatch):
    _patch_urlopen(monkeypatch, nm.urllib.error.URLError("connection refused"))
    result, err = nm.rpc_call("http://localhost:3030", "novai_getLatestBlock", None, 5.0)
    assert result is None
    assert "network" in err


def test_rpc_call_block_by_height_params(monkeypatch):
    # Verify the params object is sent as an object with a height field by
    # echoing the request body back through the fake response.
    captured = {}

    def _urlopen(req, timeout=0):
        captured["data"] = req.data
        body = json.dumps(
            {"jsonrpc": "2.0", "result": {"height": 50, "state_root": "0xdead"}, "id": 1}
        ).encode("utf-8")
        return _FakeResp(200, body)

    monkeypatch.setattr(nm.urllib.request, "urlopen", _urlopen)
    result, err = nm.rpc_call("http://localhost:3030", "novai_getBlockByHeight", {"height": 50}, 5.0)
    assert err is None
    assert result["state_root"] == "0xdead"
    sent = json.loads(captured["data"].decode("utf-8"))
    assert sent["method"] == "novai_getBlockByHeight"
    assert sent["params"] == {"height": 50}


def test_fetch_metrics_handles_unreachable(monkeypatch):
    _patch_urlopen(monkeypatch, nm.urllib.error.URLError("refused"))
    snap, err = nm.fetch_metrics("http://localhost:8080/metrics", "", "", 5.0)
    assert snap is None
    assert "network" in err


def test_fetch_metrics_parses_prometheus(monkeypatch):
    body = b"# HELP novai_committed_height x\n# TYPE novai_committed_height gauge\nnovai_committed_height 42\n"
    _patch_urlopen(monkeypatch, _FakeResp(200, body))
    snap, err = nm.fetch_metrics("http://localhost:8080/metrics", "", "", 5.0)
    assert err is None
    assert snap["novai_committed_height"] == 42.0
