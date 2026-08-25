import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useChainStatus } from "@/hooks/useChainStatus";
import snapshot from "@/data/chain-snapshot.json";

// The hook drives every chain-data surface on the site; its failure handling
// is the product. Fake timers control the poll loop; fetch is a scripted mock.

function okBlock(height: number, round = 0, txCount = 0) {
  return {
    ok: true,
    status: 200,
    json: async () => ({ jsonrpc: "2.0", id: 1, result: { height, round, tx_count: txCount, block_hash: "aa", parent_hash: "bb", state_root: "cc" } }),
  };
}

const URL = "/rpc-test";

describe("useChainStatus", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  async function tick(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  it("no rpcUrl: snapshot state forever, zero fetches", async () => {
    const spy = vi.fn();
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({}));
    await tick(30_000);
    expect(result.current.state).toBe("snapshot");
    expect(result.current.snapshot.height).toBe(snapshot.height);
    expect(spy).not.toHaveBeenCalled();
  });

  it("mounts in snapshot state (no skeleton flash), goes live on first success", async () => {
    const spy = vi.fn().mockResolvedValue(okBlock(1000));
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    expect(result.current.state).toBe("snapshot");
    await tick(1);
    expect(result.current.state).toBe("live");
    expect(result.current.height).toBe(1000);
    const body = JSON.parse(spy.mock.calls[0][1].body);
    expect(body).toMatchObject({ jsonrpc: "2.0", method: "novai_getLatestBlock", params: {} });
  });

  it("bps appears only at 3 samples and computes exactly", async () => {
    let h = 1000;
    const spy = vi.fn().mockImplementation(async () => okBlock((h += 12)));
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1); // poll 1: init, no sample
    await tick(10_000); // poll 2: sample 1
    await tick(10_000); // poll 3: sample 2
    expect(result.current.bps).toBeNull();
    await tick(10_000); // poll 4: sample 3
    expect(result.current.bps).toBeCloseTo(1.2, 5); // 12 blocks per 10s
    expect(result.current.state).toBe("live");
  });

  it("unchanged height for over 60s flips to stale, resumes to live on advance", async () => {
    const responses = [okBlock(500), okBlock(500), okBlock(500), okBlock(500), okBlock(500), okBlock(500), okBlock(500), okBlock(501)];
    const spy = vi.fn().mockImplementation(async () => responses.shift() ?? okBlock(502));
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    expect(result.current.state).toBe("live");
    await tick(65_000); // six more polls, height frozen; age passes 60s
    expect(result.current.state).toBe("stale");
    expect(result.current.ageSeconds).toBeGreaterThan(60);
    await tick(10_000); // advance arrives
    expect(result.current.state).toBe("live");
  });

  it("height going backwards resets the window: no negative rate, fresh start", async () => {
    const responses = [okBlock(9000), okBlock(9012), okBlock(9024), okBlock(9036), okBlock(50)];
    const spy = vi.fn().mockImplementation(async () => responses.shift() ?? okBlock(62));
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    await tick(30_000);
    expect(result.current.bps).toBeCloseTo(1.2, 5);
    await tick(10_000); // reset poll: height 50
    expect(result.current.height).toBe(50);
    expect(result.current.bps).toBeNull(); // window reset, never negative
    expect(result.current.state).toBe("live"); // fresh start, not an error
  });

  it("five consecutive failures with backoff lands in permanent unreachable", async () => {
    const spy = vi.fn().mockRejectedValue(new TypeError("network down"));
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    // Backoff ladder after failures: 20s, 40s, 80s, 160s; five failures total.
    await tick(1);
    await tick(20_000);
    await tick(40_000);
    await tick(80_000);
    await tick(160_000);
    expect(result.current.state).toBe("unreachable");
    expect(result.current.failures).toBe(5);
    const calls = spy.mock.calls.length;
    await tick(600_000); // permanent: no further polling this page load
    expect(spy.mock.calls.length).toBe(calls);
    expect(result.current.snapshot.height).toBe(snapshot.height);
  });

  it("HTTP 429 with a non-JSON body counts as a failure, not a crash", async () => {
    const spy = vi.fn().mockResolvedValue({ ok: false, status: 429, json: async () => { throw new SyntaxError("not json"); } });
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    expect(result.current.failures).toBe(1);
    expect(result.current.state).toBe("snapshot"); // no data yet, not unreachable
  });

  it("malformed JSON and JSON-RPC error objects count as failures", async () => {
    const responses = [
      { ok: true, status: 200, json: async () => { throw new SyntaxError("bad json"); } },
      { ok: true, status: 200, json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32600, message: "malformed" } }) },
    ];
    const spy = vi.fn().mockImplementation(async () => responses.shift() ?? okBlock(1));
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    expect(result.current.failures).toBe(1);
    await tick(20_000);
    expect(result.current.failures).toBe(2);
  });

  it("result null (no blocks yet) is NOT a failure: stays on snapshot, keeps polling", async () => {
    const spy = vi.fn().mockResolvedValue({ ok: true, status: 200, json: async () => ({ jsonrpc: "2.0", id: 1, result: null }) });
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    expect(result.current.state).toBe("snapshot");
    expect(result.current.failures).toBe(0);
    await tick(10_000);
    expect(spy.mock.calls.length).toBe(2); // normal cadence, no backoff
  });

  it("a success resets the failure counter", async () => {
    let call = 0;
    const spy = vi.fn().mockImplementation(async () => {
      call += 1;
      if (call <= 2) throw new TypeError("down");
      return okBlock(70);
    });
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    await tick(20_000);
    expect(result.current.failures).toBe(2);
    await tick(40_000);
    expect(result.current.failures).toBe(0);
    expect(result.current.state).toBe("live");
  });

  it("a request that never resolves is aborted by the 8s timeout and counted", async () => {
    const spy = vi.fn().mockImplementation((_url: string, opts: { signal: AbortSignal }) =>
      new Promise((_resolve, reject) => {
        opts.signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      })
    );
    vi.stubGlobal("fetch", spy);
    const { result } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(8_100);
    expect(result.current.failures).toBe(1);
  });

  it("unmount clears timers and aborts in-flight work", async () => {
    const spy = vi.fn().mockResolvedValue(okBlock(1));
    vi.stubGlobal("fetch", spy);
    const { unmount } = renderHook(() => useChainStatus({ rpcUrl: URL }));
    await tick(1);
    const calls = spy.mock.calls.length;
    unmount();
    await tick(120_000);
    expect(spy.mock.calls.length).toBe(calls);
  });
});
