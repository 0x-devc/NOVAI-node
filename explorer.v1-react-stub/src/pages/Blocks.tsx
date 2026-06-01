import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { rpc, BlockHeader } from "../lib/rpc";
import { formatNumber } from "../lib/format";
import HashLink from "../components/HashLink";
import Spinner from "../components/Spinner";
import ErrorState from "../components/ErrorState";

const POLL_MS = 2000;
const WINDOW_SIZE = 25;

export default function Blocks() {
  const [blocks, setBlocks] = useState<BlockHeader[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);

  useEffect(() => {
    let mounted = true;
    let timer: number | undefined;

    async function poll() {
      try {
        const latest = await rpc.getLatestBlock();
        if (!mounted) return;
        if (!latest) {
          setBlocks([]);
          setError("No blocks committed yet — the chain is still warming up.");
          return;
        }
        const start = Math.max(latest.height - WINDOW_SIZE + 1, 0);
        const heights: number[] = [];
        for (let h = latest.height; h >= start; h--) heights.push(h);

        const results = await Promise.all(
          heights.map((h) => rpc.getBlockByHeight(h)),
        );
        if (!mounted) return;
        setBlocks(results.filter((b): b is BlockHeader => b !== null));
        setError(null);
      } catch (err) {
        if (!mounted) return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (mounted) setInitialLoading(false);
      }
    }

    poll();
    timer = window.setInterval(poll, POLL_MS);
    return () => {
      mounted = false;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, []);

  if (initialLoading) return <Spinner label="Loading latest blocks…" />;
  if (error && blocks.length === 0) return <ErrorState message={error} />;

  return (
    <div className="space-y-4">
      <div className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold">Latest blocks</h1>
        <p className="text-xs text-slate-500">
          Polling every {POLL_MS / 1000}s · showing last {WINDOW_SIZE}
        </p>
      </div>

      {error && <ErrorState message={error} />}

      <div className="card overflow-x-auto">
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wider text-slate-400">
            <tr>
              <th className="py-2 pr-4">Height</th>
              <th className="py-2 pr-4">Hash</th>
              <th className="py-2 pr-4">Txs</th>
              <th className="py-2 pr-4">State root</th>
              <th className="py-2">Round</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-800">
            {blocks.map((b) => (
              <tr key={b.height} className="hover:bg-slate-900/50">
                <td className="py-2 pr-4">
                  <Link
                    to={`/blocks/${b.height}`}
                    className="text-sky-300 hover:underline tabular-nums"
                  >
                    #{formatNumber(b.height)}
                  </Link>
                </td>
                <td className="py-2 pr-4">
                  <HashLink kind="block-hash" value={b.block_hash} />
                </td>
                <td className="py-2 pr-4 tabular-nums">{b.tx_count}</td>
                <td className="py-2 pr-4 hex text-slate-400">
                  {b.state_root.slice(0, 12)}…
                </td>
                <td className="py-2 tabular-nums text-slate-400">{b.round}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
