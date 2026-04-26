import { motion } from "framer-motion";
import { Code2, GitBranch, Cpu, Database, Brain, Shield } from "lucide-react";
import ScrollSection from "./ScrollSection";

const BENTO_ITEMS = [
  {
    icon: Code2,
    title: "Rust from Scratch",
    description:
      "60,000+ lines of clean-room Rust. No forks, no copied code. Every module purpose-built.",
    span: "md:col-span-2 md:row-span-2",
    large: true,
  },
  {
    icon: Cpu,
    title: "HotStuff BFT Consensus",
    description: "Byzantine fault tolerant consensus inspired by HotStuff. Deterministic finality.",
    span: "",
    large: false,
  },
  {
    icon: Database,
    title: "Sparse Merkle Trees",
    description: "Efficient state management with cryptographic proofs.",
    span: "",
    large: false,
  },
  {
    icon: Brain,
    title: "AI Primitives",
    description:
      "AI entities as first-class protocol citizens. Native transaction types for autonomous agents.",
    span: "md:row-span-2",
    large: true,
  },
  {
    icon: GitBranch,
    title: "1,090+ Tests",
    description: "Including 105+ chaos tests. Battle-tested before mainnet.",
    span: "",
    large: false,
  },
  {
    icon: Shield,
    title: "Post-Quantum Ready",
    description: "Cryptographic abstraction layers for seamless quantum-resistant migration.",
    span: "",
    large: false,
  },
];

function BentoCard({ item, index }: { item: (typeof BENTO_ITEMS)[0]; index: number }) {
  return (
    <ScrollSection delay={index * 0.08}>
      <div
        className={`group relative rounded-2xl overflow-hidden h-full transition-all duration-500 hover:border-primary/30 ${item.span}`}
        style={{
          background: "hsla(224, 28%, 10%, 0.6)",
          border: "1px solid hsla(224, 20%, 25%, 0.3)",
          backdropFilter: "blur(20px)",
          minHeight: item.large ? 280 : 160,
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.boxShadow =
            "0 0 30px rgba(76, 111, 255, 0.15), 0 0 80px rgba(76, 111, 255, 0.08)";
          e.currentTarget.style.transform = "translateY(-2px)";
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.boxShadow = "none";
          e.currentTarget.style.transform = "translateY(0)";
        }}
      >
        <div
          className="absolute inset-0 opacity-0 group-hover:opacity-100 transition-opacity duration-500 pointer-events-none"
          style={{
            background:
              "radial-gradient(ellipse at 30% 30%, hsla(228, 100%, 62%, 0.06), transparent 60%)",
          }}
        />

        <div className="relative z-10 p-6 sm:p-8 h-full flex flex-col justify-between">
          <div>
            <div
              className="mb-4 flex h-10 w-10 items-center justify-center rounded-lg"
              style={{
                background: "hsla(228, 100%, 62%, 0.08)",
                border: "1px solid hsla(228, 100%, 62%, 0.15)",
              }}
            >
              <item.icon size={18} className="text-primary" />
            </div>
            <h3
              className={`font-display font-semibold text-foreground mb-2 ${
                item.large ? "text-2xl" : "text-lg"
              }`}
            >
              {item.title}
            </h3>
            <p
              className={`text-muted-foreground leading-relaxed ${
                item.large ? "text-sm max-w-md" : "text-xs"
              }`}
            >
              {item.description}
            </p>
          </div>

          {item.large && (
            <div className="mt-6 flex gap-1">
              {Array.from({ length: 8 }).map((_, i) => (
                <motion.div
                  key={i}
                  className="h-1 rounded-full flex-1"
                  style={{ background: `hsla(228, 100%, 62%, ${0.15 + i * 0.05})` }}
                  animate={{ scaleX: [0.3, 1, 0.3] }}
                  transition={{
                    duration: 2,
                    repeat: Infinity,
                    delay: i * 0.15,
                    ease: "easeInOut",
                  }}
                />
              ))}
            </div>
          )}
        </div>
      </div>
    </ScrollSection>
  );
}

export default function BentoGrid() {
  return (
    <div>
      <ScrollSection>
        <div className="text-center mb-12">
          <span className="pill-badge mb-6">Under the Hood</span>
          <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
            Engineering <span className="gradient-text">excellence</span>
          </h2>
          <p className="text-muted-foreground max-w-lg mx-auto">
            Every component purpose-built for the AI era.
          </p>
        </div>
      </ScrollSection>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 auto-rows-min">
        {BENTO_ITEMS.map((item, i) => (
          <BentoCard key={item.title} item={item} index={i} />
        ))}
      </div>
    </div>
  );
}
