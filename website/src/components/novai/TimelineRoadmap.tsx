import { useRef } from "react";
import { motion, useInView } from "framer-motion";
import { ChevronRight } from "lucide-react";
import ScrollSection from "./ScrollSection";

const PHASES = [
  {
    phase: "Phase 1",
    title: "Foundation",
    status: "current",
    items: [
      "Private testnet live",
      "Stress testing & attack vectors",
      "Codebase hardening",
      "Open source release",
    ],
  },
  {
    phase: "Phase 2",
    title: "Public Testnet",
    status: "upcoming",
    items: [
      "Public testnet launch",
      "Real-world condition testing",
      "Network upgrades & iteration",
      "Testnet stabilization",
    ],
  },
  {
    phase: "Phase 3",
    title: "Intelligence",
    status: "upcoming",
    items: [
      "On-chain AI agents",
      "Autonomous protocol operations",
      "Governance framework",
      "Tokenomics design",
    ],
  },
  {
    phase: "Phase 4",
    title: "Mainnet",
    status: "future",
    items: [
      "Mainnet launch",
      "Tokenomics live",
      "Hackathons & ecosystem events",
      "Developer SDK & documentation",
    ],
  },
];

function TimelineNode({
  phase,
  index,
  isLast,
}: {
  phase: (typeof PHASES)[0];
  index: number;
  isLast: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const isInView = useInView(ref, { once: true, margin: "-100px" });
  const isCurrent = phase.status === "current";

  return (
    <div ref={ref} className="relative flex gap-6 sm:gap-8 md:gap-12">
      {/* Timeline line + node */}
      <div className="flex flex-col items-center flex-shrink-0">
        <motion.div
          initial={{ scale: 0 }}
          animate={isInView ? { scale: 1 } : {}}
          transition={{ duration: 0.5, delay: 0.2, type: "spring", stiffness: 200 }}
          className={`relative w-5 h-5 rounded-full z-10 ${isCurrent ? "bg-accent" : "border-2 border-border"}`}
          style={
            isCurrent
              ? { boxShadow: "0 0 20px hsla(192, 95%, 68%, 0.5), 0 0 40px hsla(192, 95%, 68%, 0.2)" }
              : { background: "hsla(224, 28%, 10%, 0.8)" }
          }
        >
          {isCurrent && (
            <motion.div
              className="absolute inset-0 rounded-full bg-accent"
              animate={{ scale: [1, 2, 1], opacity: [0.5, 0, 0.5] }}
              transition={{ duration: 2, repeat: Infinity }}
            />
          )}
        </motion.div>

        {!isLast && (
          <motion.div
            className="w-px flex-1 min-h-[80px]"
            initial={{ scaleY: 0 }}
            animate={isInView ? { scaleY: 1 } : {}}
            transition={{ duration: 0.8, delay: 0.3 }}
            style={{
              background: isCurrent
                ? "linear-gradient(to bottom, hsla(192, 95%, 68%, 0.5), hsla(228, 100%, 62%, 0.15))"
                : "hsla(224, 20%, 25%, 0.3)",
              transformOrigin: "top",
            }}
          />
        )}
      </div>

      {/* Content */}
      <motion.div
        initial={{ opacity: 0, x: -20 }}
        animate={isInView ? { opacity: 1, x: 0 } : {}}
        transition={{ duration: 0.6, delay: 0.3, ease: [0.25, 0.46, 0.45, 0.94] }}
        className="pb-12 sm:pb-16"
      >
        <div className="flex items-center gap-3 mb-2">
          <span className="text-xs font-semibold uppercase tracking-wider text-accent">
            {phase.phase}
          </span>
          {isCurrent && (
            <span className="relative flex h-2 w-2">
              <span className="absolute inline-flex h-2 w-2 animate-ping rounded-full bg-accent opacity-75" />
              <span className="relative inline-flex h-2 w-2 rounded-full bg-accent" />
            </span>
          )}
        </div>
        <h3 className="font-display text-xl sm:text-2xl font-bold text-foreground mb-4">
          {phase.title}
        </h3>
        <div className="space-y-2.5">
          {phase.items.map((item, i) => (
            <motion.div
              key={item}
              initial={{ opacity: 0, x: -10 }}
              animate={isInView ? { opacity: 1, x: 0 } : {}}
              transition={{ duration: 0.4, delay: 0.5 + i * 0.1 }}
              className="flex items-start gap-2 text-sm text-muted-foreground"
            >
              <ChevronRight size={14} className="mt-0.5 flex-shrink-0 text-primary/60" />
              {item}
            </motion.div>
          ))}
        </div>
      </motion.div>
    </div>
  );
}

export default function TimelineRoadmap() {
  return (
    <div>
      <ScrollSection>
        <div className="text-center mb-16">
          <span className="pill-badge mb-6">Roadmap</span>
          <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
            The path <span className="gradient-text">forward</span>
          </h2>
          <p className="text-muted-foreground max-w-lg mx-auto">
            A phased approach to building the intelligent blockchain.
          </p>
        </div>
      </ScrollSection>

      <div className="max-w-2xl mx-auto pl-2 sm:pl-4">
        {PHASES.map((phase, i) => (
          <TimelineNode key={phase.phase} phase={phase} index={i} isLast={i === PHASES.length - 1} />
        ))}
      </div>
    </div>
  );
}
