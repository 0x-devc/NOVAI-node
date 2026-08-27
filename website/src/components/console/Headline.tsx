import { ReactNode } from "react";

// Headline value row: the panel's primary figure, value-first hierarchy.
export default function Headline({
  value,
  label,
  right,
}: {
  value: ReactNode;
  label: string;
  right?: ReactNode;
}) {
  return (
    <div className="px-4 py-3 flex items-end justify-between gap-4">
      <div>
        <div className="font-mono text-3xl sm:text-4xl font-light text-ink-hi tabular-nums leading-none">{value}</div>
        <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1.5">{label}</div>
      </div>
      {right && <div className="text-right pb-0.5">{right}</div>}
    </div>
  );
}
