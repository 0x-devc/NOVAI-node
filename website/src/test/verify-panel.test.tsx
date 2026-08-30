import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import VerifyPanel from "@/components/console/VerifyPanel";
import { curlFor, curlForHeightTemplate, HEIGHT_TOKEN } from "@/lib/rpc";
import snapshot from "@/data/chain-snapshot.json";

// The panel's contract: nothing fires without a click, cooldowns protect the
// node, the walk genuinely checks hash linkage, and failure is displayed.

function chainBlock(h: number, hashOf: (n: number) => string) {
  return {
    block_hash: hashOf(h),
    parent_hash: hashOf(h - 1),
    state_root: "s".repeat(64),
    height: h,
    round: 0,
    tx_count: 0,
  };
}

function mockChain(tip: number, corruptAt?: number) {
  const hashOf = (n: number) => `hash${n}`.padEnd(64, "0");
  return vi.fn().mockImplementation(async (_url: string, opts: { body: string }) => {
    const body = JSON.parse(opts.body);
    let block;
    if (body.method === "novai_getLatestBlock") block = chainBlock(tip, hashOf);
    else {
      block = chainBlock(body.params.height, hashOf);
      if (corruptAt !== undefined && body.params.height === corruptAt) {
        block = { ...block, block_hash: "corrupt".padEnd(64, "9") };
      }
    }
    return { ok: true, status: 200, json: async () => ({ jsonrpc: "2.0", id: 1, result: block }) };
  });
}

// Serves a real tip but null for every by-height query: the shape of a chain
// whose history has been pruned out from under an old height.
function mockNullByHeight(tip: number) {
  const hashOf = (n: number) => `hash${n}`.padEnd(64, "0");
  return vi.fn().mockImplementation(async (_url: string, opts: { body: string }) => {
    const body = JSON.parse(opts.body);
    const result = body.method === "novai_getLatestBlock" ? chainBlock(tip, hashOf) : null;
    return { ok: true, status: 200, json: async () => ({ jsonrpc: "2.0", id: 1, result }) };
  });
}

const URL = "/rpc-test";

describe("VerifyPanel", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  async function tick(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  it("renders with ZERO requests: no fetch on mount, scroll, or timers", async () => {
    const spy = mockChain(100);
    vi.stubGlobal("fetch", spy);
    render(<VerifyPanel rpcUrl={URL} />);
    await tick(60_000);
    expect(spy).not.toHaveBeenCalled();
  });

  it("latest block: one fetch per click, raw response includes tx_count, cooldown then re-enable", async () => {
    const spy = mockChain(4200);
    vi.stubGlobal("fetch", spy);
    render(<VerifyPanel rpcUrl={URL} />);
    const btn = screen.getByRole("button", { name: /fetch the latest block/i });
    fireEvent.click(btn);
    await tick(10);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(screen.getAllByText(/"tx_count": 0/).length).toBeGreaterThanOrEqual(2); // live response + sample exchange
    expect(screen.getByText(/"height": 4200/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    expect(spy).toHaveBeenCalledTimes(1); // cooldown swallows the second click
    await tick(3_100);
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it("verify walk: nine requests, eight verified links, completion line, 30s cooldown", async () => {
    const spy = mockChain(500);
    vi.stubGlobal("fetch", spy);
    render(<VerifyPanel rpcUrl={URL} />);
    const btn = screen.getByRole("button", { name: /verify 8 blocks/i });
    fireEvent.click(btn);
    await tick(3_000);
    expect(spy).toHaveBeenCalledTimes(9);
    expect(screen.getAllByText(/^matches$/)).toHaveLength(8);
    expect(screen.getByText(/8 blocks, 8 hash links verified in your browser/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /verify 8 blocks/i })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /verify 8 blocks/i }));
    await tick(10);
    expect(spy).toHaveBeenCalledTimes(9); // no rerun inside cooldown
    await tick(31_000);
    expect(screen.getByRole("button", { name: /verify 8 blocks/i })).toBeEnabled();
  });

  it("a corrupted link is DISPLAYED as a mismatch and the walk stops", async () => {
    const spy = mockChain(500, 497);
    vi.stubGlobal("fetch", spy);
    render(<VerifyPanel rpcUrl={URL} />);
    fireEvent.click(screen.getByRole("button", { name: /verify 8 blocks/i }));
    await tick(3_000);
    expect(screen.getByText(/HASH MISMATCH/)).toBeInTheDocument();
    expect(screen.getByText(/Verification failed at the row above/)).toBeInTheDocument();
    // latest + 499, 498, 497(corrupt): the walk stops there
    expect(spy).toHaveBeenCalledTimes(4);
  });

  it("blocked network shows the terminal-mode notice and the curl stays available", async () => {
    const spy = vi.fn().mockRejectedValue(new TypeError("cors"));
    vi.stubGlobal("fetch", spy);
    render(<VerifyPanel rpcUrl={URL} />);
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    expect(screen.getByText(/could not reach the RPC/i)).toBeInTheDocument();
    expect(screen.getAllByText(/terminal equivalent/i).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/curl -s -X POST/).length).toBeGreaterThan(0);
  });

  it("height input strips non-digits and the entered height goes into the request", async () => {
    const spy = mockChain(999);
    vi.stubGlobal("fetch", spy);
    render(<VerifyPanel rpcUrl={URL} />);
    const input = screen.getByLabelText(/block at height/i);
    fireEvent.change(input, { target: { value: "12a3!4" } });
    expect((input as HTMLInputElement).value).toBe("1234");
    fireEvent.click(screen.getByRole("button", { name: /^fetch$/i }));
    await tick(10);
    const body = JSON.parse(spy.mock.calls[0][1].body);
    expect(body.method).toBe("novai_getBlockByHeight");
    expect(body.params).toEqual({ height: 1234 });
  });

  it("sample exchange from the build snapshot renders with its capture date", () => {
    vi.stubGlobal("fetch", vi.fn());
    render(<VerifyPanel rpcUrl={URL} />);
    expect(screen.getByText(new RegExp(snapshot.capturedAt.slice(0, 10)))).toBeInTheDocument();
    expect(screen.getByText(new RegExp(`"height": ${snapshot.height}`))).toBeInTheDocument();
  });

  it("curl builder emits the exact documented invocation", () => {
    expect(curlFor("novai_getLatestBlock", {})).toBe(
      "curl -s -X POST https://rpc.novai.network \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"jsonrpc\":\"2.0\",\"method\":\"novai_getLatestBlock\",\"params\":{},\"id\":1}'"
    );
  });

  // The height input used to be seeded from the build snapshot. Retention is
  // finite, so that default drifted past the pruning horizon and a visitor's
  // first click on it answered null. These pin the fix in both halves.

  it("height input starts EMPTY and Fetch is disabled until a height is entered", () => {
    vi.stubGlobal("fetch", vi.fn());
    render(<VerifyPanel rpcUrl={URL} />);
    const input = screen.getByLabelText(/block at height/i) as HTMLInputElement;
    expect(input.value).toBe("");
    // Specifically: it is NOT seeded from the snapshot, which is what broke.
    expect(input.value).not.toBe(String(snapshot.height));
    expect(screen.getByRole("button", { name: /^fetch$/i })).toBeDisabled();
  });

  it("fetching the latest block prefills the height input with the live tip", async () => {
    vi.stubGlobal("fetch", mockChain(4_000_000));
    render(<VerifyPanel rpcUrl={URL} />);
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    expect((screen.getByLabelText(/block at height/i) as HTMLInputElement).value).toBe("4000000");
    expect(screen.getByRole("button", { name: /^fetch$/i })).not.toBeDisabled();
  });

  it("a null answer BELOW the known tip is explained as pruned, not as a failure", async () => {
    vi.stubGlobal("fetch", mockNullByHeight(1000));
    render(<VerifyPanel rpcUrl={URL} />);
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    fireEvent.change(screen.getByLabelText(/block at height/i), { target: { value: "400" } });
    fireEvent.click(screen.getByRole("button", { name: /^fetch$/i }));
    await tick(10);
    expect(screen.getByText(/no block at height/i)).toBeInTheDocument();
    expect(screen.getByText(/prunes older blocks/i)).toBeInTheDocument();
    // and it must NOT read as a transport failure
    expect(screen.queryByText(/request failed/i)).not.toBeInTheDocument();
  });

  it("a null answer ABOVE the known tip is explained as not yet produced", async () => {
    vi.stubGlobal("fetch", mockNullByHeight(1000));
    render(<VerifyPanel rpcUrl={URL} />);
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    fireEvent.change(screen.getByLabelText(/block at height/i), { target: { value: "9999" } });
    fireEvent.click(screen.getByRole("button", { name: /^fetch$/i }));
    await tick(10);
    expect(screen.getByText(/has not been produced yet/i)).toBeInTheDocument();
    expect(screen.queryByText(/prunes older blocks/i)).not.toBeInTheDocument();
  });

  it("a null answer with NO known tip names both possibilities", async () => {
    vi.stubGlobal("fetch", mockNullByHeight(1000));
    render(<VerifyPanel rpcUrl={URL} />);
    fireEvent.change(screen.getByLabelText(/block at height/i), { target: { value: "400" } });
    fireEvent.click(screen.getByRole("button", { name: /^fetch$/i }));
    await tick(10);
    expect(screen.getByText(/either above the current tip or already pruned/i)).toBeInTheDocument();
  });

  it("the by-height curl carries a substitution token until a height is known", async () => {
    vi.stubGlobal("fetch", mockChain(4_000_000));
    render(<VerifyPanel rpcUrl={URL} />);
    expect(screen.getByText(/"height":<height>/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /fetch the latest block/i }));
    await tick(10);
    expect(screen.queryByText(/"height":<height>/)).not.toBeInTheDocument();
    expect(screen.getByText(/"height":4000000/)).toBeInTheDocument();
  });

  it("the curl template cannot drift from the curl builder's format", () => {
    expect(curlForHeightTemplate()).toBe(
      curlFor("novai_getBlockByHeight", { height: 0 }).replace('"height":0', '"height":<height>')
    );
    expect(curlForHeightTemplate()).toContain(HEIGHT_TOKEN);
  });
});
