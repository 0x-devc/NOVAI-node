import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { rpc, AiEntity, MemoryObject, Signal } from "../lib/rpc";
import {
  autonomyModeName,
  capabilitiesList,
  formatBigInt,
  formatNumber,
  hexToUtf8,
  memoryTypeName,
  shortHex,
  signalTypeName,
} from "../lib/format";
import HashLink from "../components/HashLink";
import Spinner from "../components/Spinner";
import ErrorState from "../components/ErrorState";
import EmptyState from "../components/EmptyState";

const SIGNAL_LOOKBACK = 10_000;

export default function Entity() {
  const { id } = useParams<{ id: string }>();
  const [entity, setEntity] = useState<AiEntity | null>(null);
  const [memory, setMemory] = useState<MemoryObject[]>([]);
  const [signals, setSignals] = useState<Signal[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!id) return;
    if (!/^[0-9a-f]{64}$/i.test(id)) {
      setError(`Not a 32-byte entity id: ${id}`);
      setLoading(false);
      return;
    }
    const ctrl = new AbortController();
    const lower = id.toLowerCase();
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const e = await rpc.getAiEntity(lower, ctrl.signal);
        if (!e) {
          setError("Entity not found");
          return;
        }
        setEntity(e);

        // Once we know the entity exists, fetch memory + signals in parallel.
        const latest = await rpc.getLatestBlock(ctrl.signal);
        const endHeight = latest?.height ?? 0;
        const startHeight = Math.max(endHeight - SIGNAL_LOOKBACK, 0);

        const [mem, sigs] = await Promise.all([
          rpc.getMemoryObjects(lower, ctrl.signal),
          rpc
            .getSignalsByIssuer(lower, startHeight, endHeight, ctrl.signal)
            .catch(() => [] as Signal[]),
        ]);
        setMemory(mem);
        setSignals(sigs);
      } catch (err) {
        if ((err as Error).name === "AbortError") return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    })();

    return () => ctrl.abort();
  }, [id]);

  if (loading) return <Spinner label="Loading entity…" />;
  if (error) return <ErrorState message={error} />;
  if (!entity) return <ErrorState message="Entity not found" />;

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-xl font-semibold">AI Entity</h1>
        <p className="hex break-all text-xs text-slate-500 mt-1">{entity.id}</p>
      </div>

      <div className="card grid sm:grid-cols-2 gap-4">
        <Field label="Creator">
          <HashLink kind="address" value={entity.creator} />
        </Field>
        <Field label="Pubkey">
          <span className="hex">{shortHex(entity.pubkey)}</span>
        </Field>
        <Field label="Code hash">
          <span className="hex">{shortHex(entity.code_hash)}</span>
        </Field>
        <Field label="Autonomy mode">
          <span>
            {autonomyModeName(entity.autonomy_mode)}{" "}
            <span className="text-slate-500">({entity.autonomy_mode})</span>
          </span>
        </Field>
        <Field label="Capabilities">
          <div className="flex flex-wrap gap-1">
            {capabilitiesList(entity.capabilities).map((cap) => (
              <span
                key={cap}
                className="text-xs px-1.5 py-0.5 rounded bg-slate-800 text-slate-200"
              >
                {cap}
              </span>
            ))}
            <span className="text-xs text-slate-500 ml-1">
              0x{entity.capabilities.toString(16).padStart(2, "0")}
            </span>
          </div>
        </Field>
        <Field label="Active">
          {entity.is_active ? (
            <span className="text-emerald-300">yes</span>
          ) : (
            <span className="text-red-400">no (deactivated)</span>
          )}
        </Field>
        <Field label="Balance">
          <span className="tabular-nums">{formatBigInt(entity.economic_balance)}</span>
        </Field>
        <Field label="Nonce">
          <span className="tabular-nums">{formatNumber(entity.nonce)}</span>
        </Field>
        <Field label="Registered at">block {formatNumber(entity.registered_at)}</Field>
        <Field label="Last active at">block {formatNumber(entity.last_active_at)}</Field>
      </div>

      <section>
        <h2 className="text-lg font-semibold mb-2">
          Memory objects{" "}
          <span className="text-sm text-slate-500">({memory.length})</span>
        </h2>
        {memory.length === 0 ? (
          <EmptyState message="This entity owns no memory objects yet." />
        ) : (
          <div className="card overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-xs uppercase tracking-wider text-slate-400">
                <tr>
                  <th className="py-2 pr-4">Object id</th>
                  <th className="py-2 pr-4">Type</th>
                  <th className="py-2 pr-4">Size</th>
                  <th className="py-2 pr-4">Created</th>
                  <th className="py-2">Preview</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-800">
                {memory.map((m) => (
                  <tr key={m.object_id}>
                    <td className="py-2 pr-4 hex">{shortHex(m.object_id)}</td>
                    <td className="py-2 pr-4">{memoryTypeName(m.object_type)}</td>
                    <td className="py-2 pr-4 tabular-nums">{m.data_size} B</td>
                    <td className="py-2 pr-4 tabular-nums">
                      {formatNumber(m.created_at)}
                    </td>
                    <td className="py-2 text-slate-400 max-w-xs truncate">
                      {hexToUtf8(m.data)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <section>
        <h2 className="text-lg font-semibold mb-2">
          Recent signals{" "}
          <span className="text-sm text-slate-500">
            ({signals.length} in last {formatNumber(SIGNAL_LOOKBACK)} blocks)
          </span>
        </h2>
        {signals.length === 0 ? (
          <EmptyState message="No signals from this entity in the recent window." />
        ) : (
          <div className="card overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-xs uppercase tracking-wider text-slate-400">
                <tr>
                  <th className="py-2 pr-4">Block</th>
                  <th className="py-2 pr-4">Type</th>
                  <th className="py-2">Commitment</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-800">
                {signals.map((s) => (
                  <tr key={`${s.height}-${s.commitment_hash}`}>
                    <td className="py-2 pr-4 tabular-nums">
                      {formatNumber(s.height)}
                    </td>
                    <td className="py-2 pr-4">{signalTypeName(s.signal_type)}</td>
                    <td className="py-2 hex">{shortHex(s.commitment_hash)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
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
