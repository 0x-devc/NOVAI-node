import { useEffect, useRef, useState } from "react";
import snapshotData from "@/data/chain-snapshot.json";

// Live chain status against novai_getLatestBlock.
//
// State machine (approved design):
//   snapshot     initial state ALWAYS (prerender and first client paint show
//                the committed snapshot; no skeleton flash), also the no-url
//                state and the result:null (fresh chain) state
//   loading      only when no snapshot data exists (edge; forced previews)
//   live         data flowing, last observed height advance under 60s ago
//   stale        data flowing, height has not advanced for 60s
//   unreachable  5 consecutive failures; permanent for this page load
//
// Height must never appear to go backwards: a lower height (chain reset)
// resets the sample window and reads as a fresh start, never a negative rate.
// The observed rate needs at least 3 samples and is labeled as cadence
// measured in the visitor's browser, never a capability claim.

export type ChainState = "snapshot" | "loading" | "live" | "stale" | "unreachable";

export interface ChainSnapshot {
  capturedAt: string;
  height: number;
  round: number;
  txCount: number;
}

export interface ChainStatus {
  state: ChainState;
  height: number | null;
  round: number | null;
  txCount: number | null;
  bps: number | null;
  ageSeconds: number | null;
  failures: number;
  snapshot: ChainSnapshot;
}

const POLL_MS = 10_000;
const STALE_MS = 60_000;
const TIMEOUT_MS = 8_000;
const MAX_FAILURES = 5;
const WINDOW = 6;
const MIN_SAMPLES = 3;
const BACKOFF_CAP_MS = 160_000;

const SNAPSHOT: ChainSnapshot = snapshotData as ChainSnapshot;

export function useChainStatus({ rpcUrl }: { rpcUrl?: string } = {}): ChainStatus {
  const [status, setStatus] = useState<ChainStatus>({
    state: "snapshot",
    height: null,
    round: null,
    txCount: null,
    bps: null,
    ageSeconds: null,
    failures: 0,
    snapshot: SNAPSHOT,
  });

  const ref = useRef({
    lastHeight: null as number | null,
    lastAdvanceTs: null as number | null,
    lastPollTs: null as number | null,
    lastRound: null as number | null,
    lastTxCount: null as number | null,
    samples: [] as { dh: number; dtMs: number }[],
    failures: 0,
    stopped: false,
    pollTimer: 0 as ReturnType<typeof setTimeout> | 0,
    inflight: null as AbortController | null,
  });

  useEffect(() => {
    if (!rpcUrl) return;
    const r = ref.current;
    r.stopped = false;

    const compute = (): void => {
      const now = Date.now();
      const hasData = r.lastHeight !== null;
      let state: ChainState;
      if (r.failures >= MAX_FAILURES) state = "unreachable";
      else if (!hasData) state = "snapshot";
      else if (r.lastAdvanceTs !== null && now - r.lastAdvanceTs > STALE_MS) state = "stale";
      else state = "live";

      const dhSum = r.samples.reduce((a, s) => a + s.dh, 0);
      const dtSum = r.samples.reduce((a, s) => a + s.dtMs, 0);
      setStatus({
        state,
        height: r.lastHeight,
        round: r.lastRound,
        txCount: r.lastTxCount,
        bps: r.samples.length >= MIN_SAMPLES && dtSum > 0 ? dhSum / (dtSum / 1000) : null,
        ageSeconds: r.lastAdvanceTs !== null ? Math.max(0, Math.floor((now - r.lastAdvanceTs) / 1000)) : null,
        failures: r.failures,
        snapshot: SNAPSHOT,
      });
    };

    const onSuccess = (result: { height: number; round: number; tx_count: number }): void => {
      const now = Date.now();
      r.failures = 0;
      const h = result.height;
      if (r.lastHeight === null) {
        r.lastHeight = h;
        r.lastAdvanceTs = now;
      } else if (h > r.lastHeight) {
        if (r.lastPollTs !== null) {
          r.samples.push({ dh: h - r.lastHeight, dtMs: now - r.lastPollTs });
          if (r.samples.length > WINDOW) r.samples.shift();
        }
        r.lastHeight = h;
        r.lastAdvanceTs = now;
      } else if (h < r.lastHeight) {
        // Chain reset: fresh start, never a downward animation or negative rate.
        r.samples = [];
        r.lastHeight = h;
        r.lastAdvanceTs = now;
      }
      r.lastPollTs = now;
      r.lastRound = result.round;
      r.lastTxCount = result.tx_count;
    };

    const onFailure = (): void => {
      r.failures += 1;
      r.lastPollTs = null;
    };

    const poll = async (): Promise<void> => {
      if (r.stopped) return;
      if (typeof document !== "undefined" && document.hidden) {
        schedule(POLL_MS);
        return;
      }
      const controller = new AbortController();
      r.inflight = controller;
      const timeout = setTimeout(() => controller.abort(), TIMEOUT_MS);
      try {
        const res = await fetch(rpcUrl, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ jsonrpc: "2.0", method: "novai_getLatestBlock", params: {}, id: 1 }),
          signal: controller.signal,
        });
        // 429 arrives WITHOUT a JSON-RPC envelope; any non-ok is a failure
        // before json parsing is attempted.
        if (!res.ok) {
          onFailure();
        } else {
          const body = await res.json();
          if (body.error) onFailure();
          else if (body.result === null || body.result === undefined) {
            // Documented no-blocks case: a healthy server with an empty chain.
            // Not a failure; stay on snapshot and keep polling.
            r.failures = 0;
          } else if (typeof body.result.height !== "number") {
            onFailure();
          } else {
            onSuccess(body.result);
          }
        }
      } catch {
        onFailure();
      } finally {
        clearTimeout(timeout);
        r.inflight = null;
      }
      compute();
      if (r.failures >= MAX_FAILURES) return; // permanent fallback, stop polling
      schedule(r.failures > 0 ? Math.min(POLL_MS * 2 ** r.failures, BACKOFF_CAP_MS) : POLL_MS);
    };

    const schedule = (ms: number): void => {
      if (r.stopped) return;
      r.pollTimer = setTimeout(poll, ms);
    };

    const onVisible = (): void => {
      if (typeof document === "undefined" || document.hidden || r.stopped) return;
      // Resume with a fresh sample window so the rate never spans the gap.
      r.samples = [];
      r.lastPollTs = null;
      clearTimeout(r.pollTimer);
      void poll();
    };

    const ticker = setInterval(compute, 1000);
    if (typeof document !== "undefined") document.addEventListener("visibilitychange", onVisible);
    void poll();

    return () => {
      r.stopped = true;
      clearTimeout(r.pollTimer);
      clearInterval(ticker);
      r.inflight?.abort();
      if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onVisible);
    };
  }, [rpcUrl]);

  return status;
}
