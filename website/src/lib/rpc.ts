// Shared JSON-RPC helper for the verify panel (and later the network
// section). Read-only methods only; the site never submits transactions.

export interface BlockHeader {
  block_hash: string;
  parent_hash: string;
  state_root: string;
  height: number;
  round: number;
  tx_count: number;
}

export const PUBLIC_RPC_URL = "https://rpc.novai.network";

export type RpcOutcome<T> =
  | { ok: true; result: T; elapsedMs: number }
  | { ok: false; kind: "blocked" | "rate-limited" | "rpc-error" | "malformed" | "timeout"; detail: string };

export async function rpcCall<T>(
  url: string,
  method: string,
  params: Record<string, unknown>,
  timeoutMs = 8000
): Promise<RpcOutcome<T>> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const started = Date.now();
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", method, params, id: 1 }),
      signal: controller.signal,
    });
    if (res.status === 429) {
      // The node's rate limiter answers without a JSON-RPC envelope.
      return { ok: false, kind: "rate-limited", detail: "HTTP 429 from the node rate limiter" };
    }
    if (!res.ok) return { ok: false, kind: "rpc-error", detail: `HTTP ${res.status}` };
    let body: { result?: T; error?: { code: number; message: string } };
    try {
      body = await res.json();
    } catch {
      return { ok: false, kind: "malformed", detail: "response was not JSON" };
    }
    if (body.error) return { ok: false, kind: "rpc-error", detail: `${body.error.code}: ${body.error.message}` };
    if (body.result === null || body.result === undefined)
      return { ok: false, kind: "rpc-error", detail: "empty result" };
    return { ok: true, result: body.result, elapsedMs: Date.now() - started };
  } catch (err) {
    if (err instanceof DOMException && err.name === "AbortError")
      return { ok: false, kind: "timeout", detail: `no response within ${timeoutMs / 1000}s` };
    // fetch TypeError: network down or CORS-blocked; the browser cannot tell
    // them apart and neither can this code, so the copy covers both.
    return { ok: false, kind: "blocked", detail: "browser could not reach the RPC" };
  } finally {
    clearTimeout(timer);
  }
}

/** The exact terminal equivalent of a call, shown beside every action. */
export function curlFor(method: string, params: Record<string, unknown>): string {
  const body = JSON.stringify({ jsonrpc: "2.0", method, params, id: 1 });
  return `curl -s -X POST ${PUBLIC_RPC_URL} \\\n  -H 'Content-Type: application/json' \\\n  -d '${body}'`;
}

export const delay = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
