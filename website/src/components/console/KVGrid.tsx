import { ReactNode } from "react";

// Dense grid of related figures under a headline: hairline-divided cells,
// no per-cell borders, values first, labels small mono beneath.
export function KVGrid({ cols = 4, children }: { cols?: 2 | 3 | 4; children: ReactNode }) {
  const colClass = { 2: "sm:grid-cols-2", 3: "sm:grid-cols-3", 4: "sm:grid-cols-4" }[cols];
  // sm:w-max for the same reason DataTable is not w-full: cells size to their
  // content rather than stretching across the viewport. max-w-full keeps that
  // from overflowing when the content is genuinely wide.
  return (
    <div className={`grid ${colClass} divide-y divide-line-subtle sm:w-max sm:max-w-full sm:divide-y-0 sm:divide-x`}>
      {children}
    </div>
  );
}

export function KV({
  value,
  label,
  note,
  tone = "hi",
}: {
  value: ReactNode;
  label: string;
  note?: string;
  tone?: "hi" | "low";
}) {
  return (
    <div className="px-4 py-2.5">
      <div className={`font-mono text-lg tabular-nums leading-none ${tone === "hi" ? "text-ink-hi" : "text-ink-low"}`}>
        {value}
      </div>
      <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1">{label}</div>
      {note && <div className="text-[11px] text-ink-low mt-0.5">{note}</div>}
    </div>
  );
}
