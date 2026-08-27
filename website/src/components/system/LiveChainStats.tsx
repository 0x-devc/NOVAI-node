import type { ChainStatus } from "@/hooks/useChainStatus";
import StatusDot from "./StatusDot";
import Panel, { Caption, PanelRow } from "@/components/console/Panel";
import Headline from "@/components/console/Headline";
import { KVGrid, KV } from "@/components/console/KVGrid";
import { fmtAge } from "@/lib/format";
import type { ReactNode } from "react";

// The network console: the four designed chain-data states in the dense
// data-console vocabulary. This layout is the prototype for the site's
// #network section. Values first, labels small mono, one bordered surface,
// hairlines inside. The status dot is the one cyan element. This component
// never fabricates data.

function Measuring() {
  return <span className="text-ink-low text-sm">measuring</span>;
}

export default function LiveChainStats({ status, meta }: { status: ChainStatus; meta?: ReactNode }) {
  const { state, snapshot } = status;
  const live = state === "live" || state === "stale";
  const height = live ? status.height : state === "loading" ? null : snapshot.height;
  const round = live ? status.round : state === "loading" ? null : snapshot.round;
  const txCount = live ? status.txCount : state === "loading" ? null : snapshot.txCount;
  const blockHash = live ? status.blockHash : state === "loading" ? null : snapshot.blockHash;
  const stateRoot = live ? status.stateRoot : state === "loading" ? null : snapshot.stateRoot;
  const capturedDate = snapshot.capturedAt.slice(0, 10);
  const shortHex = (h: string) => `${h.slice(0, 12)}..${h.slice(-8)}`;

  return (
    <Panel
      title="network"
      meta={
        <>
          {meta}
          <StatusDot state={state} />
        </>
      }
    >
      <Headline
        value={height !== null ? height.toLocaleString() : <span className="text-ink-low">connecting</span>}
        label={live ? "block height, novai_getLatestBlock" : "block height, build snapshot"}
        right={
          <span className="font-mono text-[11px] text-ink-low">
            {state === "live" && status.ageSeconds !== null && `last block ${fmtAge(status.ageSeconds)} ago`}
            {state === "snapshot" && `captured ${capturedDate}`}
            {state === "unreachable" && `rpc unreachable, snapshot ${capturedDate}`}
            {state === "stale" && status.ageSeconds !== null && `held ${fmtAge(status.ageSeconds)}`}
          </span>
        }
      />

      {state === "stale" && (
        <PanelRow>
          <p className="text-sm text-ink-hi">
            Testnet paused. Last block {height?.toLocaleString()},{" "}
            {status.ageSeconds !== null ? fmtAge(status.ageSeconds) : ""} ago.
          </p>
          <p className="text-[11px] text-ink-low mt-0.5">Pauses are normal for an actively developed testnet.</p>
        </PanelRow>
      )}

      <KVGrid cols={3}>
        <KV
          value={live ? (status.bps !== null ? status.bps.toFixed(2) : <Measuring />) : <span className="text-ink-low">idle</span>}
          label="observed cadence, blk/s"
          note="measured in this browser"
        />
        <KV
          value={round !== null ? String(round) : "?"}
          label="latest round"
          note="0 is a first-attempt commit"
        />
        <KV
          value={txCount !== null ? String(txCount) : "?"}
          label="tx in latest block"
          note={live ? "novai_getLatestBlock" : "build snapshot"}
        />
      </KVGrid>

      {(blockHash || stateRoot) && (
        <PanelRow className="grid sm:grid-cols-2 gap-x-6 gap-y-1">
          {blockHash && (
            <div className="min-w-0">
              <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low">block hash </span>
              <span className="font-mono text-xs text-ink-mid break-all" title={blockHash}>
                {shortHex(blockHash)}
              </span>
            </div>
          )}
          {stateRoot && (
            <div className="min-w-0">
              <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low">state root </span>
              <span className="font-mono text-xs text-ink-mid break-all" title={stateRoot}>
                {shortHex(stateRoot)}
              </span>
            </div>
          )}
        </PanelRow>
      )}

      <Caption>
        {state === "live" &&
          "Quorum active: height is advancing, so at least 3 of 4 validators are signing. Blocks commit only on a quorum certificate. Polled every 2s while this panel is on screen, 10s off screen, paused in hidden tabs."}
        {state === "stale" && "Quorum indicator paused with the chain."}
        {(state === "snapshot" || state === "unreachable") &&
          "Live quorum indicator runs when this page can reach the RPC."}
        {state === "loading" && "Quorum indicator derives from height advancing between polls."}
      </Caption>
    </Panel>
  );
}
