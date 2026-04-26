import { useState } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { Brain, Shield, Zap, Layers } from "lucide-react";
import ScrollSection from "./ScrollSection";

const FEATURES = [
  {
    icon: Brain,
    title: "AI-Aware Consensus",
    description:
      "AI entity signals integrated directly into the consensus message format. The protocol carries intelligence as a first-class data type — enabling real-time monitoring, autonomous threat response, and adaptive network behavior.",
    visual: "consensus",
  },
  {
    icon: Shield,
    title: "Post-Quantum Architecture",
    description:
      "Core architecture designed for post-quantum migration. Built with cryptographic abstraction layers that enable seamless transition to quantum-resistant primitives as NIST standards mature.",
    visual: "quantum",
  },
  {
    icon: Zap,
    title: "Self-Adjusting Protocol",
    description:
      "The network autonomously responds to congestion, detects inefficiencies, and deprioritises suspicious transactions — all within governance-approved bounds. Real-time adaptation with built-in safety limits.",
    visual: "adaptive",
  },
  {
    icon: Layers,
    title: "Ultra-Scalable",
    description:
      "HotStuff-inspired BFT consensus with Sparse Merkle Tree state management for fast, predictable, low-cost transactions.",
    visual: "scalable",
  },
];

function ConsensusVisual() {
  return (
    <div className="relative w-full h-full flex items-center justify-center">
      <motion.div
        animate={{ scale: [1, 1.1, 1], opacity: [0.8, 1, 0.8] }}
        transition={{ duration: 3, repeat: Infinity, ease: "easeInOut" }}
        className="absolute w-20 h-20 rounded-full flex items-center justify-center"
        style={{
          background: "radial-gradient(circle, hsla(228, 100%, 62%, 0.3), hsla(228, 100%, 62%, 0.05))",
          border: "1px solid hsla(228, 100%, 62%, 0.3)",
          boxShadow: "0 0 40px hsla(228, 100%, 62%, 0.2)",
        }}
      >
        <Brain size={32} className="text-primary" />
      </motion.div>
      {[0, 1, 2, 3, 4].map((i) => {
        const angle = (i * 72 - 90) * (Math.PI / 180);
        const radius = 100;
        return (
          <motion.div
            key={i}
            className="absolute w-10 h-10 rounded-full flex items-center justify-center"
            style={{
              left: `calc(50% + ${Math.cos(angle) * radius}px - 20px)`,
              top: `calc(50% + ${Math.sin(angle) * radius}px - 20px)`,
              background: "hsla(224, 28%, 10%, 0.8)",
              border: "1px solid hsla(192, 95%, 68%, 0.3)",
            }}
            animate={{ scale: [1, 1.15, 1] }}
            transition={{ duration: 2, repeat: Infinity, delay: i * 0.3 }}
          >
            <div className="w-3 h-3 rounded-full bg-accent/60" />
          </motion.div>
        );
      })}
    </div>
  );
}

function QuantumVisual() {
  return (
    <div className="relative w-full h-full flex items-center justify-center">
      {[0, 1, 2].map((i) => (
        <motion.div
          key={i}
          className="absolute rounded-full"
          style={{
            width: 160 - i * 40,
            height: 160 - i * 40,
            border: `1px solid hsla(228, 100%, 62%, ${0.1 + i * 0.1})`,
          }}
          animate={{ rotate: i % 2 === 0 ? 360 : -360 }}
          transition={{ duration: 20 - i * 5, repeat: Infinity, ease: "linear" }}
        >
          <div
            className="absolute w-3 h-3 rounded-full"
            style={{
              top: 0,
              left: "50%",
              transform: "translate(-50%, -50%)",
              background: "hsla(192, 95%, 68%, 0.6)",
              boxShadow: "0 0 10px hsla(192, 95%, 68%, 0.4)",
            }}
          />
        </motion.div>
      ))}
      <Shield size={32} className="text-primary relative z-10" />
    </div>
  );
}

function AdaptiveVisual() {
  return (
    <div className="relative w-full h-full flex items-end justify-center gap-2 pb-12">
      {Array.from({ length: 12 }).map((_, i) => (
        <motion.div
          key={i}
          className="w-3 rounded-t-sm"
          style={{ background: `hsla(${228 + i * 3}, 100%, ${55 + i * 2}%, 0.6)` }}
          animate={{
            height: [
              20 + Math.sin(i * 0.8) * 30 + 20,
              20 + Math.cos(i * 0.5) * 50 + 20,
              20 + Math.sin(i * 0.8) * 30 + 20,
            ],
          }}
          transition={{ duration: 2 + i * 0.2, repeat: Infinity, ease: "easeInOut" }}
        />
      ))}
    </div>
  );
}

function ScalableVisual() {
  return (
    <div className="relative w-full h-full flex items-center justify-center">
      <div className="grid grid-cols-4 gap-2">
        {Array.from({ length: 16 }).map((_, i) => (
          <motion.div
            key={i}
            className="w-10 h-10 rounded-lg flex items-center justify-center"
            style={{
              background: "hsla(224, 28%, 10%, 0.8)",
              border: "1px solid hsla(224, 20%, 25%, 0.3)",
            }}
            animate={{ opacity: [0.3, 0.8, 0.3] }}
            transition={{ duration: 2, repeat: Infinity, delay: i * 0.12 }}
          >
            <span className="text-[9px] font-mono text-primary/40">#{i + 1}</span>
          </motion.div>
        ))}
      </div>
    </div>
  );
}

const VISUALS: Record<string, React.FC> = {
  consensus: ConsensusVisual,
  quantum: QuantumVisual,
  adaptive: AdaptiveVisual,
  scalable: ScalableVisual,
};

export default function StickyFeatures() {
  const [active, setActive] = useState(0);
  const ActiveVisual = VISUALS[FEATURES[active].visual];

  return (
    <div>
      <ScrollSection>
        <div className="text-center mb-16">
          <span className="pill-badge mb-6">Core Technology</span>
          <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
            Built to <span className="gradient-text">think</span>
          </h2>
          <p className="text-muted-foreground max-w-lg mx-auto">
            Four pillars powering the intelligent network.
          </p>
        </div>
      </ScrollSection>

      <ScrollSection delay={0.1}>
        <div className="grid lg:grid-cols-2 gap-12 items-center">
          {/* Left: Tabs */}
          <div className="space-y-3">
            {FEATURES.map((feature, i) => {
              const isActive = active === i;
              return (
                <button
                  key={feature.title}
                  onClick={() => setActive(i)}
                  className={`w-full text-left rounded-2xl p-6 transition-all duration-300 ${
                    isActive ? "" : "opacity-50 hover:opacity-80"
                  }`}
                  style={{
                    background: isActive ? "hsla(224, 28%, 10%, 0.6)" : "transparent",
                    border: isActive
                      ? "1px solid hsla(228, 100%, 62%, 0.2)"
                      : "1px solid transparent",
                    boxShadow: isActive ? "0 0 20px rgba(76, 111, 255, 0.08)" : "none",
                  }}
                >
                  <div className="flex items-center gap-4 mb-2">
                    <div
                      className="flex h-10 w-10 items-center justify-center rounded-lg flex-shrink-0"
                      style={{
                        background: isActive
                          ? "hsla(228, 100%, 62%, 0.1)"
                          : "hsla(224, 28%, 10%, 0.4)",
                        border: `1px solid ${
                          isActive ? "hsla(228, 100%, 62%, 0.2)" : "hsla(224, 20%, 25%, 0.3)"
                        }`,
                      }}
                    >
                      <feature.icon
                        size={18}
                        className={isActive ? "text-primary" : "text-muted-foreground"}
                      />
                    </div>
                    <h3
                      className={`font-display text-lg font-semibold ${
                        isActive ? "text-foreground" : "text-muted-foreground"
                      }`}
                    >
                      {feature.title}
                    </h3>
                  </div>
                  <AnimatePresence>
                    {isActive && (
                      <motion.p
                        initial={{ opacity: 0, height: 0 }}
                        animate={{ opacity: 1, height: "auto" }}
                        exit={{ opacity: 0, height: 0 }}
                        transition={{ duration: 0.3 }}
                        className="text-sm text-muted-foreground leading-relaxed pl-14 overflow-hidden"
                      >
                        {feature.description}
                      </motion.p>
                    )}
                  </AnimatePresence>
                </button>
              );
            })}
          </div>

          {/* Right: Visual (desktop) */}
          <div className="relative h-[350px] hidden lg:block">
            <div
              className="absolute inset-0 rounded-2xl"
              style={{
                background: "hsla(224, 28%, 10%, 0.3)",
                border: "1px solid hsla(224, 20%, 25%, 0.2)",
              }}
            />
            <AnimatePresence mode="wait">
              <motion.div
                key={active}
                initial={{ opacity: 0, scale: 0.95 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.95 }}
                transition={{ duration: 0.4 }}
                className="absolute inset-0"
              >
                {ActiveVisual && <ActiveVisual />}
              </motion.div>
            </AnimatePresence>
          </div>

          {/* Visual (mobile) */}
          <div className="relative h-[250px] lg:hidden">
            <div
              className="absolute inset-0 rounded-2xl"
              style={{
                background: "hsla(224, 28%, 10%, 0.3)",
                border: "1px solid hsla(224, 20%, 25%, 0.2)",
              }}
            />
            <AnimatePresence mode="wait">
              <motion.div
                key={active}
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.3 }}
                className="absolute inset-0"
              >
                {ActiveVisual && <ActiveVisual />}
              </motion.div>
            </AnimatePresence>
          </div>
        </div>
      </ScrollSection>
    </div>
  );
}
