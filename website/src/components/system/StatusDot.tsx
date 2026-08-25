import type { ChainState } from "@/hooks/useChainStatus";

// The one cyan element per viewport: the live-state indicator. Every other
// state renders in its own status hue or stays neutral.
const TREATMENT: Record<ChainState, { dot: string; word: string }> = {
  live: { dot: "bg-live animate-pulse", word: "live" },
  stale: { dot: "bg-warnx", word: "paused" },
  snapshot: { dot: "border border-line-strong bg-transparent", word: "snapshot" },
  unreachable: { dot: "bg-errorx", word: "unreachable" },
  loading: { dot: "bg-surface-3 animate-pulse", word: "connecting" },
};

export default function StatusDot({ state }: { state: ChainState }) {
  const t = TREATMENT[state];
  return (
    <span className="inline-flex items-center gap-2">
      <span className={`inline-block h-2.5 w-2.5 rounded-full ${t.dot}`} aria-hidden="true" />
      <span className="font-mono text-label uppercase tracking-[0.05em] text-ink-mid">{t.word}</span>
    </span>
  );
}
