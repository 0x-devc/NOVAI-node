import { ReactNode } from "react";
import { m } from "framer-motion";
import { fadeRise, staggerChildren } from "@/lib/motion";
import { usePrefersReducedMotion } from "@/hooks/usePrefersReducedMotion";

// In-view-once fade-rise wrapper. Under reduced motion the children render
// statically at final position; nothing is ever hidden from a non-JS reader
// because the server path renders children unwrapped (Gate 7 wires that).
export default function Reveal({
  children,
  stagger = false,
  className = "",
}: {
  children: ReactNode;
  stagger?: boolean;
  className?: string;
}) {
  const reduced = usePrefersReducedMotion();
  const variants = reduced ? fadeRise.reduced : fadeRise.full;
  return (
    <m.div
      initial="hidden"
      whileInView="visible"
      viewport={{ once: true, margin: "-80px" }}
      variants={variants}
      transition={stagger ? staggerChildren(reduced) : undefined}
      className={className}
    >
      {children}
    </m.div>
  );
}
