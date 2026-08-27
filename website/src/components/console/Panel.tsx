import { ReactNode } from "react";

// The one bordered surface in the console vocabulary. Everything inside
// separates with hairlines and shared gutters, never nested borders.
export default function Panel({
  title,
  meta,
  children,
  className = "",
}: {
  title: string;
  meta?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`border border-line rounded-md bg-surface-1 ${className}`}>
      <div className="flex flex-wrap items-center justify-between gap-2 px-4 py-2 border-b border-line-subtle">
        <span className="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-low">{title}</span>
        {meta && <span className="flex items-center gap-3">{meta}</span>}
      </div>
      <div className="divide-y divide-line-subtle">{children}</div>
    </div>
  );
}

/** A plain content row inside a Panel. */
export function PanelRow({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`px-4 py-2.5 ${className}`}>{children}</div>;
}

/** Foot-of-panel provenance or explanation, small and consistent. */
export function Caption({ children }: { children: ReactNode }) {
  return (
    <div className="px-4 py-2">
      <p className="text-[11px] leading-relaxed text-ink-low">{children}</p>
    </div>
  );
}
