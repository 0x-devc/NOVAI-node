import { useEffect, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { rpc, TxReceipt } from "../lib/rpc";
import { formatNumber } from "../lib/format";
import HashLink from "../components/HashLink";
import Spinner from "../components/Spinner";
import ErrorState from "../components/ErrorState";

export default function TxDetail() {
  const { txid } = useParams<{ txid: string }>();
  const [receipt, setReceipt] = useState<TxReceipt | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!txid) return;
    if (!/^[0-9a-f]{64}$/i.test(txid)) {
      setError(`Not a 32-byte txid: ${txid}`);
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const r = await rpc.getTransaction(txid.toLowerCase(), ctrl.signal);
        setReceipt(r);
        if (!r) setError("Transaction not found (may still be in mempool)");
      } catch (err) {
        if ((err as Error).name === "AbortError") return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    })();

    return () => ctrl.abort();
  }, [txid]);

  if (loading) return <Spinner label="Loading transaction…" />;
  if (error) return <ErrorState message={error} />;
  if (!receipt) return <ErrorState message="Transaction not found" />;

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold">Transaction</h1>

      <div className="card grid sm:grid-cols-2 gap-4">
        <Field label="Txid">
          <span className="hex break-all">{txid}</span>
        </Field>
        <Field label="Block">
          <Link
            to={`/blocks/${receipt.block_height}`}
            className="text-sky-300 hover:underline tabular-nums"
          >
            #{formatNumber(receipt.block_height)}
          </Link>
          <span className="text-slate-500"> · index {receipt.tx_index}</span>
        </Field>
        <Field label="From">
          <HashLink kind="address" value={receipt.from} full />
        </Field>
        <Field label="Nonce">
          <span className="tabular-nums">{receipt.nonce}</span>
        </Field>
        <Field label="Fee">
          <span className="tabular-nums">{formatNumber(receipt.fee)}</span>
        </Field>
        <Field label="Payload size">
          <span className="tabular-nums">{receipt.payload_len} bytes</span>
        </Field>
      </div>

      <p className="text-xs text-slate-500">
        The receipt confirms inclusion. Execution outcome (success vs.
        application-level rejection) isn't stored separately — query the
        relevant state (account balance, entity record, etc.) to confirm
        side-effects landed.
      </p>
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
