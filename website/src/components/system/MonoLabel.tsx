import { ReactNode } from "react";

// Uppercase mono label token: 12-14px fluid, +0.05em tracking. The technical
// voice of the system (stat labels, kickers, provenance rails).
export default function MonoLabel({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <span className={`font-mono text-label uppercase tracking-[0.05em] text-ink-low ${className}`}>
      {children}
    </span>
  );
}
