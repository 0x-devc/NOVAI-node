import { m } from "framer-motion";
import MonoLabel from "./MonoLabel";
import { statSettle } from "@/lib/motion";
import { usePrefersReducedMotion } from "@/hooks/usePrefersReducedMotion";

// Stat tile: display-family value over an uppercase mono label, with a
// one-line provenance microcopy. The value renders at its FINAL number in the
// markup (never counts from zero; crawlers and no-JS readers see the real
// value) and settles in with a short opacity+blur pass.
export default function StatTile({
  value,
  label,
  provenance,
}: {
  value: string;
  label: string;
  provenance: string;
}) {
  const reduced = usePrefersReducedMotion();
  const variants = reduced ? statSettle.reduced : statSettle.full;
  return (
    <div className="min-h-24 px-6 py-5 first:pl-0 last:pr-0">
      <m.div
        initial="hidden"
        whileInView="visible"
        viewport={{ once: true, margin: "-40px" }}
        variants={variants}
        className="font-display text-stat font-light text-ink-hi tabular-nums"
      >
        {value}
      </m.div>
      <div className="mt-2">
        <MonoLabel>{label}</MonoLabel>
      </div>
      <p className="text-xs text-ink-low mt-1.5">{provenance}</p>
    </div>
  );
}
