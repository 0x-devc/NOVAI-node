import { useEffect, useRef, useState } from "react";
import { rpc, BlockHeader } from "../lib/rpc";
import { formatNumber } from "../lib/format";
import Spinner from "../components/Spinner";
import ErrorState from "../components/ErrorState";

const POLL_MS = 2000;
const SAMPLE_DEPTH = 100; // recent blocks summed for tx count

interface RatePoint {
  height: number;
  ts: number;
}

export default function Stats() {
  const [latest, setLatest] = useState<BlockHeader | null>(null);
  const [recentTxCount, setRecentTxCount] = useState<number | null>(null);
  const [bps, setBps] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const ratePoints = useRef<RatePoint[]>([]);

  useEffect(() => {
    let mounted = true;

    async function tick() {
      try {
        const head = await rpc.getLatestBlock();
        if (!mounted) return;
        setLatest(head);
        if (!head) {
          setError("Chain not producing blocks yet.");
          return;
        }

        // Compute blocks/sec from a sliding window of (height, timestamp)
        // points. Two points → simple slope; older points get evicted after
        // 30 seconds.
        const now = Date.now();
        ratePoints.current.push({ height: head.height, ts: now });
        ratePoints.current = ratePoints.current.filter(
          (p) => now - p.ts < 30_000,
        );
        if (ratePoints.current.length >= 2) {
          const oldest = ratePoints.current[0];
          const dHeight = head.height - oldest.height;
          const dSeconds = (now - oldest.ts) / 1000;
          if (dSeconds > 0) setBps(dHeight / dSeconds);
        }

        // Fetch the last SAMPLE_DEPTH block headers; sum tx_count.
        const start = Math.max(head.height - SAMPLE_DEPTH + 1, 0);
        const heights: number[] = [];
        for (let h = head.height; h >= start; h--) heights.push(h);
        const headers = await Promise.all(
          heights.map((h) => rpc.getBlockByHeight(h)),
        );
        if (!mounted) return;
        const total = headers.reduce(
          (acc, b) => acc + (b ? b.tx_count : 0),
          0,
        );
        setRecentTxCount(total);
        setError(null);
      } catch (err) {
        if (!mounted) return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (mounted) setLoading(false);
      }
    }

    tick();
    const timer = window.setInterval(tick, POLL_MS);
    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, []);

  if (loading) return <Spinner label="Sampling chain…" />;
  if (error && !latest) return <ErrorState message={error} />;

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold">Network stats</h1>

      {error && <ErrorState message={error} />}

      <div className="grid sm:grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard label="Latest height" value={latest ? formatNumber(latest.height) : "—"} />
        <StatCard
          label="Blocks / sec"
          value={bps === null ? "sampling…" : bps.toFixed(2)}
          hint="rolling 30s sample"
        />
        <StatCard
          label={`Txs in last ${SAMPLE_DEPTH} blocks`}
          value={recentTxCount === null ? "—" : formatNumber(recentTxCount)}
        />
        <StatCard
          label="Validators"
          value="4"
          hint="devnet preset · no validator-set RPC yet"
        />
      </div>

      <p className="text-xs text-slate-500">
        Validator count is hardcoded for the devnet (4) — the node doesn't yet
        expose a validator-set RPC, so the explorer can't probe it. Total-tx-ever
        also isn't available without walking history; the panel above shows the
        windowed total over the last {SAMPLE_DEPTH} committed blocks instead.
      </p>
    </div>
  );
}

function StatCard({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="card">
      <p className="label">{label}</p>
      <p className="stat mt-2 tabular-nums">{value}</p>
      {hint && <p className="text-xs text-slate-500 mt-1">{hint}</p>}
    </div>
  );
}
