import { ReactNode } from "react";

// Real tables for anything that is a list. Compact mono rows, numeric
// columns right-aligned, hairline row dividers, no cell borders.
//
// Deliberately NOT w-full: the page uses the viewport width, but a table
// stretched to it spreads three columns across two thousand pixels of dead
// space. Sizing to content keeps columns adjacent, which is the whole point of
// the dense register. The overflow-x wrapper handles the opposite case.
export interface Column {
  key: string;
  label: string;
  align?: "left" | "right";
}

export default function DataTable({
  columns,
  children,
}: {
  columns: Column[];
  children: ReactNode;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="text-xs font-mono">
        <thead>
          <tr className="border-b border-line-subtle">
            {columns.map((c) => (
              <th
                key={c.key}
                scope="col"
                className={`px-4 py-1.5 font-medium text-[10px] uppercase tracking-[0.08em] text-ink-low ${
                  c.align === "right" ? "text-right" : "text-left"
                }`}
              >
                {c.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-line-subtle">{children}</tbody>
      </table>
    </div>
  );
}

export function Td({
  children,
  align = "left",
  className = "",
}: {
  children: ReactNode;
  align?: "left" | "right";
  className?: string;
}) {
  return (
    <td className={`px-4 py-1.5 tabular-nums ${align === "right" ? "text-right" : "text-left"} ${className}`}>
      {children}
    </td>
  );
}
