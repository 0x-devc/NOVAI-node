import { useEffect, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { rpc, BalanceInfo, AiEntity } from "../lib/rpc";
import { formatBigInt, formatNumber } from "../lib/format";
import Spinner from "../components/Spinner";
import ErrorState from "../components/ErrorState";

export default function Account() {
  const { address } = useParams<{ address: string }>();
  const [balance, setBalance] = useState<BalanceInfo | null>(null);
  const [maybeEntity, setMaybeEntity] = useState<AiEntity | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!address) return;
    if (!/^[0-9a-f]{64}$/i.test(address)) {
      setError(`Not a 32-byte address: ${address}`);
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    const lower = address.toLowerCase();
    setLoading(true);
    setError(null);

    (async () => {
      try {
        // Probe in parallel: balance always returns (zeroes if account doesn't
        // exist). getAiEntity returns null if not an entity id.
        const [bal, ent] = await Promise.all([
          rpc.getBalance(lower, ctrl.signal),
          rpc.getAiEntity(lower, ctrl.signal).catch(() => null),
        ]);
        setBalance(bal);
        setMaybeEntity(ent);
      } catch (err) {
        if ((err as Error).name === "AbortError") return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    })();

    return () => ctrl.abort();
  }, [address]);

  if (loading) return <Spinner label="Loading account…" />;
  if (error) return <ErrorState message={error} />;
  if (!balance) return <ErrorState message="Account not found" />;

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold">Account</h1>

      <div className="card space-y-3">
        <div>
          <p className="label">Address</p>
          <p className="hex break-all text-sm mt-1">{address}</p>
        </div>
        <div className="grid sm:grid-cols-2 gap-4 pt-2">
          <div>
            <p className="label">Balance</p>
            <p className="stat tabular-nums mt-1">
              {formatBigInt(balance.balance)}
            </p>
          </div>
          <div>
            <p className="label">Next nonce</p>
            <p className="stat tabular-nums mt-1">
              {formatNumber(balance.nonce)}
            </p>
          </div>
        </div>
      </div>

      {maybeEntity && (
        <div className="card border-sky-900 bg-sky-950/20">
          <p className="text-sm">
            This 32-byte value is also a registered AI entity.{" "}
            <Link
              to={`/entity/${maybeEntity.id}`}
              className="text-sky-300 hover:underline"
            >
              View entity record →
            </Link>
          </p>
        </div>
      )}

      <div className="card text-xs text-slate-500">
        <p>
          <span className="font-semibold text-slate-300">Transaction history</span>{" "}
          for this address is not yet available. Per-address tx indexing is on
          the roadmap; today only by-block and by-txid lookups are exposed via
          JSON-RPC.
        </p>
      </div>
    </div>
  );
}
