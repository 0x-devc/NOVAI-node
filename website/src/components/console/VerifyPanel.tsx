import { useEffect, useRef, useState } from "react";
import Panel, { Caption, PanelRow } from "@/components/console/Panel";
import DataTable, { Td } from "@/components/console/DataTable";
import chainSnapshot from "@/data/chain-snapshot.json";
import {
  rpcCall,
  curlFor,
  curlForHeightTemplate,
  delay,
  type BlockHeader,
  type RpcOutcome,
} from "@/lib/rpc";

// The verify panel: the visitor's own browser calls the live chain and, in
// the centerpiece action, re-derives a consensus property by checking that
// eight consecutive parent-hash links actually chain.
//
// Rules this component enforces by construction:
// - Every request is user-triggered. Nothing fires on mount, on scroll, or on
//   a timer. The RPC node is also a validator; the panel treats it gently.
// - Client-side cooldowns: 30s after a verify walk, 3s after a single fetch.
// - tx_count renders exactly as returned. It is a field, not a message.
// - A hash mismatch is displayed, never swallowed: a check that cannot fail
//   visibly is not a check.

const VERIFY_DEPTH = 8;
const REQUEST_SPACING_MS = 150;
const VERIFY_COOLDOWN_MS = 30_000;
const FETCH_COOLDOWN_MS = 3_000;

const SNAPSHOT_RESULT: BlockHeader = {
  block_hash: chainSnapshot.blockHash,
  parent_hash: chainSnapshot.parentHash,
  state_root: chainSnapshot.stateRoot,
  height: chainSnapshot.height,
  round: chainSnapshot.round,
  tx_count: chainSnapshot.txCount,
};

type FetchState =
  | { phase: "idle" }
  | { phase: "running" }
  | { phase: "done"; block: BlockHeader; elapsedMs: number }
  // A null answer is not a failure. Which side of the chain the height falls on
  // is the useful fact, so it is carried and explained rather than discarded.
  | { phase: "empty"; height: number; tip: number | null }
  | { phase: "failed"; kind: string; detail: string };

interface LinkRow {
  height: number;
  hash: string;
  ok: boolean;
  expected?: string;
  got?: string;
}

type WalkState =
  | { phase: "idle" }
  | { phase: "running"; tip: BlockHeader | null; links: LinkRow[] }
  | { phase: "done"; tip: BlockHeader; links: LinkRow[]; totalMs: number }
  | { phase: "mismatch"; tip: BlockHeader; links: LinkRow[] }
  | { phase: "failed"; kind: string; detail: string; links: LinkRow[] };

const short = (hex: string): string => `${hex.slice(0, 10)}..${hex.slice(-6)}`;

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
      className="text-xs font-mono text-brand-text hover:underline"
    >
      {copied ? "copied" : "copy"}
    </button>
  );
}

function CurlBlock({ cmd, note }: { cmd: string; note?: string }) {
  return (
    <div className="bg-surface-0 px-4 py-2">
      <div className="flex items-center justify-between">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low">terminal equivalent</span>
        <CopyButton text={cmd} />
      </div>
      <pre className="text-xs font-mono text-ink-mid overflow-x-auto whitespace-pre mt-1">{cmd}</pre>
      {note && <p className="mt-1 text-[11px] text-ink-low">{note}</p>}
    </div>
  );
}

function ResponseBlock({ block, elapsedMs, label }: { block: BlockHeader; elapsedMs?: number; label: string }) {
  return (
    <div className="bg-surface-0 px-4 py-2">
      <div className="flex items-center justify-between">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low">{label}</span>
        {elapsedMs !== undefined && <span className="text-xs font-mono text-ink-low">{elapsedMs}ms</span>}
      </div>
      <pre className="text-xs font-mono text-ink-mid overflow-x-auto whitespace-pre mt-1">
        {JSON.stringify({ jsonrpc: "2.0", result: block, id: 1 }, null, 2)}
      </pre>
    </div>
  );
}

function FailureNotice({ kind, detail }: { kind: string; detail: string }) {
  return (
    <div className="bg-surface-0 px-4 py-2.5 space-y-1">
      {kind === "blocked" ? (
        <>
          <p className="text-sm text-ink-hi">Your browser could not reach the RPC.</p>
          <p className="text-xs text-ink-low">
            Expected until the RPC allows this origin. The chain is still there: run the curl command from a
            terminal and you get the same response this panel would show.
          </p>
        </>
      ) : kind === "rate-limited" ? (
        <p className="text-sm text-ink-mid">The node rate limiter answered. Wait a moment and try again.</p>
      ) : kind === "empty" ? (
        <p className="text-sm text-ink-mid">
          The node answered with null for one of these blocks. That block is no longer retained: a node serves
          recent history only and prunes older blocks.
        </p>
      ) : (
        <p className="text-sm text-ink-mid">
          Request failed: {kind} ({detail})
        </p>
      )}
    </div>
  );
}

// A null answer to a height query is a fact about the chain, not a failure of
// the request. Which side of the tip the height falls on decides which fact,
// and saying so is more useful to a reader than reporting an empty result.
function EmptyNotice({ height, tip }: { height: number; tip: number | null }) {
  const pruned = tip !== null && height < tip;
  const future = tip !== null && height > tip;
  return (
    <div className="bg-surface-0 px-4 py-2.5 space-y-1">
      <p className="text-sm text-ink-hi tabular-nums">
        No block at height {height.toLocaleString()}. The node answered, and the result was null.
      </p>
      {pruned && (
        <p className="text-xs text-ink-low tabular-nums">
          The chain tip is {tip.toLocaleString()}. A node serves recent history only and prunes older blocks, so
          heights far behind the tip are no longer retrievable from the RPC. Plan any indexer around that window.
        </p>
      )}
      {future && (
        <p className="text-xs text-ink-low tabular-nums">
          The chain tip is {tip.toLocaleString()}, so that block has not been produced yet.
        </p>
      )}
      {tip === null && (
        <p className="text-xs text-ink-low">
          That height is either above the current tip or already pruned: a node serves recent history only. Fetch
          the latest block to see where the tip is.
        </p>
      )}
    </div>
  );
}

export default function VerifyPanel({ rpcUrl }: { rpcUrl?: string }) {
  const [latest, setLatest] = useState<FetchState>({ phase: "idle" });
  const [byHeight, setByHeight] = useState<FetchState>({ phase: "idle" });
  // Deliberately empty. Seeding this from the build snapshot prefilled a height
  // that the node had long since pruned, so the first click on the default
  // value answered null. The tip is the only height guaranteed to be servable,
  // and it is not known until the visitor asks for it.
  const [heightInput, setHeightInput] = useState("");
  const [knownTip, setKnownTip] = useState<number | null>(null);
  const [walk, setWalk] = useState<WalkState>({ phase: "idle" });
  const [cooldowns, setCooldowns] = useState<{ [k: string]: number }>({});
  const [, forceTick] = useState(0);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    const t = setInterval(() => forceTick((n) => n + 1), 1000);
    return () => {
      alive.current = false;
      clearInterval(t);
    };
  }, []);

  const now = Date.now();
  const coolingFor = (key: string): number => Math.max(0, Math.ceil(((cooldowns[key] ?? 0) - now) / 1000));
  const startCooldown = (key: string, ms: number): void =>
    setCooldowns((c) => ({ ...c, [key]: Date.now() + ms }));

  const canRun = rpcUrl !== undefined;

  const runLatest = async (): Promise<void> => {
    if (!canRun || latest.phase === "running" || coolingFor("latest") > 0) return;
    setLatest({ phase: "running" });
    const out: RpcOutcome<BlockHeader> = await rpcCall(rpcUrl, "novai_getLatestBlock", {});
    if (!alive.current) return;
    startCooldown("latest", FETCH_COOLDOWN_MS);
    if (out.ok) {
      setLatest({ phase: "done", block: out.result, elapsedMs: out.elapsedMs });
      setKnownTip(out.result.height);
      setHeightInput(String(out.result.height));
    } else setLatest({ phase: "failed", kind: out.kind, detail: out.detail });
  };

  const runByHeight = async (): Promise<void> => {
    if (!canRun || byHeight.phase === "running" || coolingFor("byHeight") > 0) return;
    const h = Number(heightInput);
    if (!Number.isInteger(h) || h < 0) return;
    setByHeight({ phase: "running" });
    const out: RpcOutcome<BlockHeader> = await rpcCall(rpcUrl, "novai_getBlockByHeight", { height: h });
    if (!alive.current) return;
    startCooldown("byHeight", FETCH_COOLDOWN_MS);
    if (out.ok) setByHeight({ phase: "done", block: out.result, elapsedMs: out.elapsedMs });
    else if (out.kind === "empty") setByHeight({ phase: "empty", height: h, tip: knownTip });
    else setByHeight({ phase: "failed", kind: out.kind, detail: out.detail });
  };

  const runWalk = async (): Promise<void> => {
    if (!canRun || walk.phase === "running" || coolingFor("walk") > 0) return;
    const started = Date.now();
    setWalk({ phase: "running", tip: null, links: [] });
    const tipOut: RpcOutcome<BlockHeader> = await rpcCall(rpcUrl, "novai_getLatestBlock", {});
    if (!alive.current) return;
    if (!tipOut.ok) {
      startCooldown("walk", VERIFY_COOLDOWN_MS);
      setWalk({ phase: "failed", kind: tipOut.kind, detail: tipOut.detail, links: [] });
      return;
    }
    const tip = tipOut.result;
    setKnownTip(tip.height);
    let expectedHash = tip.parent_hash;
    const links: LinkRow[] = [];
    for (let i = 1; i <= VERIFY_DEPTH; i++) {
      await delay(REQUEST_SPACING_MS);
      if (!alive.current) return;
      const h = tip.height - i;
      const out: RpcOutcome<BlockHeader> = await rpcCall(rpcUrl, "novai_getBlockByHeight", { height: h });
      if (!alive.current) return;
      if (!out.ok) {
        startCooldown("walk", VERIFY_COOLDOWN_MS);
        setWalk({ phase: "failed", kind: out.kind, detail: out.detail, links });
        return;
      }
      const block = out.result;
      const ok = block.block_hash === expectedHash && block.height === h;
      links.push({
        height: h,
        hash: block.block_hash,
        ok,
        ...(ok ? {} : { expected: expectedHash, got: block.block_hash }),
      });
      setWalk({ phase: "running", tip, links: [...links] });
      if (!ok) {
        startCooldown("walk", VERIFY_COOLDOWN_MS);
        setWalk({ phase: "mismatch", tip, links: [...links] });
        return;
      }
      expectedHash = block.parent_hash;
    }
    startCooldown("walk", VERIFY_COOLDOWN_MS);
    setWalk({ phase: "done", tip, links, totalMs: Date.now() - started });
  };

  const walkCooldown = coolingFor("walk");

  return (
    <Panel title="query the chain">
      <Caption>
        Real requests from your browser to the public RPC. Nothing runs until you click, every call shows its
        terminal equivalent, and the responses are raw.
      </Caption>

      <PanelRow>
        <div className="flex flex-wrap items-center gap-3">
          <button
            onClick={() => void runLatest()}
            disabled={!canRun || latest.phase === "running" || coolingFor("latest") > 0}
            className="rounded-md px-3 py-1.5 text-sm font-semibold text-white disabled:opacity-40"
            style={{ background: "hsl(var(--brand))" }}
          >
            {latest.phase === "running" ? "fetching" : "Fetch the latest block"}
          </button>
          {coolingFor("latest") > 0 && (
            <span className="text-[11px] font-mono text-ink-low">again in {coolingFor("latest")}s</span>
          )}
        </div>
      </PanelRow>
      {latest.phase === "done" && <ResponseBlock block={latest.block} elapsedMs={latest.elapsedMs} label="live response" />}
      {latest.phase === "failed" && <FailureNotice kind={latest.kind} detail={latest.detail} />}
      <CurlBlock cmd={curlFor("novai_getLatestBlock", {})} />

      <PanelRow>
        <div className="flex flex-wrap items-center gap-3">
          <label className="text-sm text-ink-mid" htmlFor="vp-height">
            Block at height
          </label>
          <input
            id="vp-height"
            value={heightInput}
            onChange={(e) => setHeightInput(e.target.value.replace(/[^0-9]/g, ""))}
            inputMode="numeric"
            placeholder="fetch the tip first"
            className="w-44 rounded-md border border-line bg-surface-0 px-2.5 py-1 text-sm font-mono text-ink-hi tabular-nums placeholder:text-ink-low placeholder:text-xs"
          />
          <button
            onClick={() => void runByHeight()}
            disabled={
              !canRun || heightInput === "" || byHeight.phase === "running" || coolingFor("byHeight") > 0
            }
            className="rounded-md border border-line px-3 py-1 text-sm font-semibold text-ink-hi disabled:opacity-40 hover:border-line-strong"
          >
            {byHeight.phase === "running" ? "fetching" : "Fetch"}
          </button>
          {coolingFor("byHeight") > 0 && (
            <span className="text-[11px] font-mono text-ink-low">again in {coolingFor("byHeight")}s</span>
          )}
        </div>
      </PanelRow>
      {byHeight.phase === "done" && (
        <ResponseBlock block={byHeight.block} elapsedMs={byHeight.elapsedMs} label="live response" />
      )}
      {byHeight.phase === "empty" && <EmptyNotice height={byHeight.height} tip={byHeight.tip} />}
      {byHeight.phase === "failed" && <FailureNotice kind={byHeight.kind} detail={byHeight.detail} />}
      {heightInput === "" ? (
        <CurlBlock
          cmd={curlForHeightTemplate()}
          note="Substitute a height near the current tip. Older heights answer null once the node has pruned them."
        />
      ) : (
        <CurlBlock cmd={curlFor("novai_getBlockByHeight", { height: Number(heightInput) })} />
      )}

      <PanelRow>
        <div className="flex flex-wrap items-center gap-3">
          <button
            onClick={() => void runWalk()}
            disabled={!canRun || walk.phase === "running" || walkCooldown > 0}
            className="rounded-md px-3 py-1.5 text-sm font-semibold text-white disabled:opacity-40"
            style={{ background: "hsl(var(--brand))", boxShadow: "var(--glow-2)" }}
          >
            {walk.phase === "running" ? "verifying" : `Verify ${VERIFY_DEPTH} blocks in your browser`}
          </button>
          {walkCooldown > 0 && <span className="text-[11px] font-mono text-ink-low">again in {walkCooldown}s</span>}
          <span className="text-[11px] text-ink-low">
            {VERIFY_DEPTH + 1} sequential requests, spaced. Each block must name the previous block's hash as its
            parent.
          </span>
        </div>
      </PanelRow>

      {(walk.phase === "running" || walk.phase === "done" || walk.phase === "mismatch") && walk.tip && (
        <div className="bg-surface-0">
          <div className="px-4 py-1.5 border-b border-line-subtle">
            <span className="font-mono text-xs text-ink-mid tabular-nums">
              tip {walk.tip.height.toLocaleString()} hash {short(walk.tip.block_hash)} tx_count {walk.tip.tx_count}
            </span>
          </div>
          <DataTable
            columns={[
              { key: "h", label: "height", align: "right" },
              { key: "hash", label: "block hash" },
              { key: "check", label: "parent check" },
            ]}
          >
            {walk.links.map((l) => (
              <tr key={l.height}>
                <Td align="right">{l.height.toLocaleString()}</Td>
                <Td className="text-ink-low">{short(l.hash)}</Td>
                <Td className={l.ok ? "text-ink-mid" : "text-errorx-text"}>
                  {l.ok
                    ? "matches"
                    : `HASH MISMATCH: child says ${short(l.expected ?? "")}, chain returned ${short(l.got ?? "")}`}
                </Td>
              </tr>
            ))}
          </DataTable>
          {walk.phase === "done" && (
            <p className="px-4 py-2 text-sm text-ink-hi border-t border-line-subtle">
              {VERIFY_DEPTH} blocks, {VERIFY_DEPTH} hash links verified in your browser just now, in{" "}
              {(walk.totalMs / 1000).toFixed(1)}s.
            </p>
          )}
          {walk.phase === "mismatch" && (
            <p className="px-4 py-2 text-sm text-errorx-text border-t border-line-subtle">
              Verification failed at the row above. That is the point of a real check: it can fail. If you see
              this, the chain served inconsistent headers and I want to know.
            </p>
          )}
        </div>
      )}
      {walk.phase === "failed" && <FailureNotice kind={walk.kind} detail={walk.detail} />}

      <ResponseBlock
        block={SNAPSHOT_RESULT}
        label={`sample exchange, captured at build ${chainSnapshot.capturedAt.slice(0, 10)}`}
      />
    </Panel>
  );
}
