import { type ReactNode } from "react";

interface GlowCardProps {
  children: ReactNode;
  className?: string;
}

export default function GlowCard({ children, className = "" }: GlowCardProps) {
  return (
    <div
      className={`group relative rounded-2xl border border-border/30 transition-all duration-300 hover:border-border/40 ${className}`}
      style={{
        background: "hsla(224, 28%, 10%, 0.6)",
        backdropFilter: "blur(20px)",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.boxShadow =
          "0 0 20px rgba(76, 111, 255, 0.3), 0 0 60px rgba(76, 111, 255, 0.15), 0 0 100px rgba(125, 211, 252, 0.08)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.boxShadow = "none";
      }}
    >
      <div className="relative z-10">{children}</div>
    </div>
  );
}
