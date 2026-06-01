import { useEffect, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { rpc, BlockHeader } from "../lib/rpc";
import { formatNumber } from "../lib/format";
import HashLink from "../components/HashLink";
import Spinner from "../components/Spinner";
import ErrorState from "../components/ErrorState";

export default function BlockDetail() {
  const { heightOrHash } = useParams<{ heightOrHash: string }>();
  const [block, setBlock] = useState<BlockHeader | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!heightOrHash) return;
    const ctrl = new AbortController();
    setLoading(true);
    setError(null);

    (async () => {
      try {
        let result: BlockHeader | null;
        if (/^\d+$/.test(heightOrHash)) {
          result = await rpc.getBlockByHeight(Number(heightOrHash), ctrl.signal);
        } else if (/^[0-9a-f]{64}$/i.test(heightOrHash)) {
          result = await rpc.getBlockByHash(heightOrHash.toLowerCase(), ctrl.signal);
        } else {
          setError(`Not a height or 32-byte hash: ${heightOrHash}`);
          return;
        }
        setBlock(result);
        if (!result) setError("Block not found");
      } catch (err) {
        if ((err as Error).name === "AbortError") return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    })();

    return () => ctrl.abort();
  }, [heightOrHash]);

  if (loading) return <Spinner label="Loading block…" />;
  if (error) return <ErrorState message={error} />;
  if (!block) return <ErrorState message="Block not found" />;

  return (
    <div className="space-y-4">
      <div className="flex items-baseline gap-3">
        <h1 className="text-xl font-semibold">
          Block #{formatNumber(block.height)}
        </h1>
        <span className="text-xs text-slate-500">round {block.round}</span>
      </div>

      <div className="card grid sm:grid-cols-2 gap-4">
        <Field label="Height">
          <span className="tabular-nums">{formatNumber(block.height)}</span>
        </Field>
        <Field label="Round">
          <span className="tabular-nums">{block.round}</span>
        </Field>
        <Field label="Tx count">
          <span className="tabular-nums">{block.tx_count}</span>
        </Field>
        <Field label="Block hash">
          <span className="hex break-all">{block.block_hash}</span>
        </Field>
        <Field label="Parent hash">
          {block.height > 0 ? (
            <HashLink kind="block-hash" value={block.parent_hash} full />
          ) : (
            <span className="hex break-all text-slate-500">
              {block.parent_hash}
            </span>
          )}
        </Field>
        <Field label="State root">
          <span className="hex break-all">{block.state_root}</span>
        </Field>
      </div>

      <div className="flex gap-3 text-sm">
        {block.height > 0 && (
          <Link
            to={`/blocks/${block.height - 1}`}
            className="text-sky-300 hover:underline"
          >
            ← Block #{formatNumber(block.height - 1)}
          </Link>
        )}
        <Link
          to={`/blocks/${block.height + 1}`}
          className="text-sky-300 hover:underline"
        >
          Block #{formatNumber(block.height + 1)} →
        </Link>
      </div>

      {block.tx_count > 0 && (
        <p className="text-xs text-slate-500">
          The node doesn't currently expose per-block tx lists via JSON-RPC. To
          look up an individual tx, paste its txid into the search bar.
        </p>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <p className="label">{label}</p>
      <div className="text-sm mt-1">{children}</div>
    </div>
  );
}
