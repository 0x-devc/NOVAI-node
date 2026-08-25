import type { ChainStatus } from "@/hooks/useChainStatus";
import StatusDot from "./StatusDot";
import MonoLabel from "./MonoLabel";
import { fmtAge } from "@/lib/format";

// The four designed chain-data states on one presentational surface. Values
// come from the hook (or a forced preview object in the specimen); this
// component never fabricates data. Height digits stay ink-hi: the status dot
// is the section's one cyan element (accent scarcity).

function Skeleton({ w = "w-24" }: { w?: string }) {
  return <span className={`inline-block h-8 ${w} rounded bg-surface-3 animate-pulse align-middle`} aria-hidden="true" />;
}

function Tile({ value, label, note }: { value: React.ReactNode; label: string; note: string }) {
  return (
    <div className="px-6 py-5 first:pl-0 last:pr-0">
      <div className="font-mono text-stat font-light text-ink-hi tabular-nums">{value}</div>
      <div className="mt-2">
        <MonoLabel>{label}</MonoLabel>
      </div>
      <p className="text-xs text-ink-low mt-1.5">{note}</p>
    </div>
  );
}

export default function LiveChainStats({ status }: { status: ChainStatus }) {
  const { state, snapshot } = status;
  const showLiveData = state === "live" || state === "stale";
  const height = showLiveData ? status.height : state === "loading" ? null : snapshot.height;
  const round = showLiveData ? status.round : state === "loading" ? null : snapshot.round;
  const capturedDate = snapshot.capturedAt.slice(0, 10);

  return (
    <div className="border border-line rounded-lg bg-surface-1">
      <div className="flex flex-wrap items-center justify-between gap-3 px-6 py-3 border-b border-line-subtle">
        <span className="inline-flex items-center gap-4">
          <StatusDot state={state} />
          <span className="text-xs text-ink-low font-mono">polled every 10s</span>
        </span>
        <span className="text-xs text-ink-low">
          {state === "live" && status.ageSeconds !== null && `last block ${fmtAge(status.ageSeconds)} ago`}
          {state === "snapshot" && `snapshot, captured ${capturedDate}`}
          {state === "unreachable" && `RPC unreachable from your network, showing snapshot captured ${capturedDate}`}
          {state === "loading" && "waiting for first response"}
        </span>
      </div>

      {state === "stale" && (
        <div className="px-6 py-4 border-b border-line-subtle">
          <p className="text-bodyx text-ink-hi">
            Testnet paused. Last block {height?.toLocaleString()}, {status.ageSeconds !== null ? fmtAge(status.ageSeconds) : ""} ago.
          </p>
          <p className="text-sm text-ink-low mt-1">Pauses are normal for an actively developed testnet.</p>
        </div>
      )}

      <div className="grid sm:grid-cols-3 divide-y sm:divide-y-0 sm:divide-x divide-line-subtle px-6">
        <Tile
          value={height !== null ? height.toLocaleString() : <Skeleton />}
          label="Block height"
          note={showLiveData ? "novai_getLatestBlock, live" : "build-time snapshot"}
        />
        <Tile
          value={
            state === "live" || state === "stale" ? (
              status.bps !== null ? status.bps.toFixed(2) : <Skeleton w="w-16" />
            ) : (
              <span className="text-ink-low">idle</span>
            )
          }
          label="Observed cadence"
          note="blocks per second, measured in this browser"
        />
        <Tile
          value={round !== null ? String(round) : <Skeleton w="w-10" />}
          label="Latest round"
          note="round 0 means a first-attempt commit, no view change"
        />
      </div>

      <div className="px-6 py-3 border-t border-line-subtle">
        <p className="text-xs text-ink-low">
          {state === "live" &&
            "Quorum active: height is advancing, so at least 3 of 4 validators are signing. Blocks commit only on a quorum certificate."}
          {state === "stale" && "Quorum indicator paused with the chain."}
          {(state === "snapshot" || state === "unreachable") &&
            "Live quorum indicator runs when this page can reach the RPC."}
          {state === "loading" && "Quorum indicator derives from height advancing between polls."}
        </p>
      </div>
    </div>
  );
}
