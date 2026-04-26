"use client";

import { useCallback, useRef } from "react";

export default function CardGlow({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);

  const handleMouseMove = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    el.style.setProperty("--glow-x", `${e.clientX - rect.left}px`);
    el.style.setProperty("--glow-y", `${e.clientY - rect.top}px`);
  }, []);

  return (
    <div
      ref={ref}
      className={`card-glow ${className}`}
      onMouseMove={handleMouseMove}
      style={{ position: "relative", overflow: "hidden" }}
    >
      <div
        className="card-glow-border"
        style={{
          position: "absolute",
          inset: 0,
          pointerEvents: "none",
          borderRadius: "inherit",
          opacity: 0,
          transition: "opacity 300ms ease",
          background: "radial-gradient(circle 200px at var(--glow-x, 50%) var(--glow-y, 50%), rgba(76,111,255,0.25), transparent)",
        }}
      />
      {children}
    </div>
  );
}
