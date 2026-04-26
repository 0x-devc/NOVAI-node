import { motion } from "framer-motion";
import ScrollSection from "@/components/novai/ScrollSection";
import Footer from "@/components/novai/Footer";

const PILLARS = [
  {
    title: "Self-Adjusting",
    description:
      "The protocol detects congestion patterns, identifies inefficiencies, and responds to emerging threats — autonomously adjusting parameters within governance-approved bounds to maintain peak performance. Not waiting for committee votes. Acting within safe limits.",
  },
  {
    title: "AI-Driven Security",
    description:
      "AI entities monitor the network in real time, scoring transaction patterns for anomalies. Suspicious activity is automatically deprioritised — not censored — with full transparency for validator operators. Proactive defense built into the protocol's DNA.",
  },
  {
    title: "Post-Quantum Architecture",
    description:
      "Core architecture designed with cryptographic abstraction layers for quantum-resistant migration. Not a bolted-on afterthought — a foundational design decision that ensures NOVAInetwork can adopt post-quantum primitives as standards mature.",
  },
  {
    title: "Ultra-Scalable",
    description:
      "HotStuff-inspired BFT consensus with Sparse Merkle Tree state management. Deterministic transaction execution means predictable performance.",
  },
  {
    title: "Developer-First",
    description:
      "Purpose-built transaction types for AI entity management, value transfer, and autonomous agent operations. AI entities can originate transactions, respond to network conditions, and operate as independent economic actors within the protocol.",
  },
];

export default function VisionPage() {
  return (
    <div className="relative">
      {/* Hero */}
      <section className="relative min-h-[70vh] flex items-center overflow-hidden">
        <div className="absolute inset-0 mesh-gradient pointer-events-none" />
        <div className="absolute inset-0 grid-bg-fine opacity-15 pointer-events-none" />

        <div className="section-container relative z-10 pt-28 pb-20">
          <motion.div
            initial={{ opacity: 0, y: 30 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.8 }}
            className="max-w-3xl"
          >
            <span className="pill-badge mb-8">Our Vision</span>
            <h1 className="font-display text-5xl font-bold leading-[1.1] sm:text-6xl lg:text-7xl mb-8">
              Intelligence is not a{" "}
              <span className="gradient-text">feature.</span>
              <br />
              It&apos;s the <span className="gradient-text">foundation.</span>
            </h1>
            <p className="text-xl text-muted-foreground leading-relaxed max-w-2xl">
              NOVAInetwork is building toward a mainstream, AI-integrated Layer-1 blockchain.
              Designed for real-world scale, high performance, and a developer-first experience.
              AI entities operate as autonomous protocol primitives — monitoring, responding, and adapting within governance-approved bounds.
            </p>
          </motion.div>
        </div>
      </section>

      {/* Pull Quote */}
      <section className="relative py-20">
        <div className="gradient-divider" />
        <div className="section-container py-20">
          <ScrollSection>
            <blockquote className="text-center">
              <p className="font-display text-3xl sm:text-4xl font-light italic text-foreground/80 leading-relaxed max-w-3xl mx-auto">
                &ldquo;The goal is long-term, mainstream utility across dApps,
                infrastructure, and{" "}
                <span className="gradient-text font-medium not-italic">real-world systems.</span>&rdquo;
              </p>
            </blockquote>
          </ScrollSection>
        </div>
        <div className="gradient-divider" />
      </section>

      {/* Pillars */}
      <section className="relative py-32">
        <div className="absolute inset-0 mesh-gradient pointer-events-none opacity-30" />
        <div className="section-container relative z-10">
          <ScrollSection>
            <div className="mb-20">
              <span className="pill-badge mb-6">Design Principles</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl">
                Built to <span className="gradient-text">evolve</span>
              </h2>
            </div>
          </ScrollSection>

          <div className="space-y-6">
            {PILLARS.map((pillar, i) => (
              <ScrollSection key={pillar.title} delay={i * 0.08}>
                <div className="glass-card-hover glow-border rounded-2xl p-8 sm:p-10 flex flex-col sm:flex-row gap-6 sm:gap-12 items-start">
                  <div className="flex-shrink-0">
                    <span className="font-display text-6xl font-bold gradient-text opacity-30">
                      {String(i + 1).padStart(2, "0")}
                    </span>
                  </div>
                  <div>
                    <h3 className="font-display text-2xl font-semibold mb-4 text-foreground">
                      {pillar.title}
                    </h3>
                    <p className="text-muted-foreground leading-relaxed">
                      {pillar.description}
                    </p>
                  </div>
                </div>
              </ScrollSection>
            ))}
          </div>
        </div>
      </section>

      <Footer />
    </div>
  );
}
