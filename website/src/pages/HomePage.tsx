import { Link } from "react-router-dom";
import { motion } from "framer-motion";
import {
  ArrowRight,
  Shield,
  Zap,
  Brain,
  Layers,
  Code2,
  ChevronRight,
  Coins,
} from "lucide-react";

import GlowOrb from "@/components/novai/GlowOrb";
import AnimatedCounter from "@/components/novai/AnimatedCounter";
import ScrollSection from "@/components/novai/ScrollSection";
import GlowCard from "@/components/novai/GlowCard";
import Footer from "@/components/novai/Footer";

const FEATURES = [
  {
    icon: Brain,
    title: "AI-Aware Consensus",
    description:
      "AI entity signals integrated directly into the consensus message format. The protocol carries intelligence as a first-class data type — enabling real-time monitoring, autonomous threat response, and adaptive network behavior.",
  },
  {
    icon: Shield,
    title: "Post-Quantum Architecture",
    description:
      "Core architecture designed for post-quantum migration. Built with cryptographic abstraction layers that enable seamless transition to quantum-resistant primitives as NIST standards mature.",
  },
  {
    icon: Zap,
    title: "Self-Adjusting Protocol",
    description:
      "The network autonomously responds to congestion, detects inefficiencies, and deprioritises suspicious transactions — all within governance-approved bounds. Real-time adaptation with built-in safety limits.",
  },
  {
    icon: Layers,
    title: "Ultra-Scalable",
    description:
      "HotStuff-inspired BFT consensus with Sparse Merkle Tree state management for fast, predictable, low-cost transactions.",
  },
];

const STATS: { value?: number; suffix?: string; label: string; prefix?: string; textOnly?: string }[] = [
  { textOnly: "Active", label: "Private Testnet Running" },
  { value: 4, suffix: "", label: "Active Validators", prefix: "" },
  { value: 1090, suffix: "+", label: "Tests Passing", prefix: "" },
  { value: 50000, suffix: "+", label: "Mock Transactions", prefix: "" },
];

const ROADMAP = [
  {
    phase: "Phase 1",
    title: "Foundation",
    status: "current",
    items: ["Private testnet live", "Stress testing & attack vectors", "Codebase hardening", "Open source release"],
  },
  {
    phase: "Phase 2",
    title: "Public Testnet",
    status: "upcoming",
    items: ["Public testnet launch", "Real-world condition testing", "Network upgrades & iteration", "Testnet stabilization"],
  },
  {
    phase: "Phase 3",
    title: "Intelligence",
    status: "upcoming",
    items: ["On-chain AI agents", "Autonomous protocol operations", "Governance framework", "Tokenomics design"],
  },
  {
    phase: "Phase 4",
    title: "Mainnet",
    status: "future",
    items: ["Mainnet launch", "Tokenomics live", "Hackathons & ecosystem events", "Developer SDK & documentation"],
  },
];

export default function HomePage() {
  return (
    <div className="relative">

      {/* ═══════════════ HERO ═══════════════ */}
      <section className="relative min-h-screen flex items-center overflow-hidden">
        {/* Mesh gradient bg */}
        <div className="absolute inset-0 mesh-gradient pointer-events-none" />
        <div className="absolute inset-0 grid-bg opacity-30 pointer-events-none" />

        {/* Nebula glows */}
        <div
          className="absolute pointer-events-none"
          style={{
            width: 900,
            height: 900,
            top: "5%",
            right: "-15%",
            background: "radial-gradient(circle, hsla(228, 100%, 62%, 0.1), transparent 60%)",
            filter: "blur(80px)",
          }}
        />
        <div
          className="absolute pointer-events-none"
          style={{
            width: 700,
            height: 700,
            bottom: "10%",
            left: "-10%",
            background: "radial-gradient(circle, hsla(192, 95%, 68%, 0.07), transparent 60%)",
            filter: "blur(60px)",
          }}
        />

        <div className="section-container relative z-10 flex w-full flex-col items-center gap-16 pt-24 pb-20 lg:flex-row lg:justify-between">
          {/* Left */}
          <motion.div
            initial={{ opacity: 0, y: 40 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.8, ease: [0.25, 0.46, 0.45, 0.94] }}
            className="max-w-2xl"
          >
            <div className="pill-badge mb-8">
              <span className="relative flex h-2 w-2">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-75" />
                <span className="relative inline-flex h-2 w-2 rounded-full bg-accent" />
              </span>
              Building the Future of Blockchain
            </div>

            <h1 className="font-display font-bold leading-[1.1] tracking-tight mb-6">
              <span className="block gradient-text text-5xl sm:text-6xl lg:text-7xl">
                NOVAInetwork
              </span>
              <span className="block text-foreground text-3xl sm:text-4xl lg:text-5xl mt-2">
                The AI-Integrated Layer 1 Blockchain
              </span>
              <span
                className="block text-xl sm:text-2xl mt-3 italic"
                style={{ color: "hsl(192, 95%, 68%)" }}
              >
                The intelligent network is awakening.
              </span>
            </h1>

            <p className="text-lg text-muted-foreground leading-relaxed mb-4 max-w-xl">
              NOVAInetwork is a standalone Layer-1 blockchain with AI embedded directly into the protocol layer — not bolted on as an afterthought.
            </p>

            <p className="text-sm text-muted-foreground mb-8 max-w-lg">
              Built from scratch in Rust. HotStuff BFT consensus. Deterministic execution. AI agents as first-class protocol citizens.
            </p>

            <div className="flex flex-wrap gap-4 mb-8">
              <Link to="/documents" className="btn-primary no-underline">
                Documentation <ArrowRight size={16} />
              </Link>
              <Link to="/socials" className="btn-ghost no-underline">
                Join Community
              </Link>
            </div>

            <p className="text-xs text-muted-foreground/60">
              Status: Pre-mainnet · Private testnet live · Public testnet coming soon · Built from scratch in Rust
            </p>
          </motion.div>

          {/* Right Orb */}
          <motion.div
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ duration: 1.2, delay: 0.3, ease: [0.25, 0.46, 0.45, 0.94] }}
            className="relative flex-shrink-0 hidden lg:block"
          >
            <div className="relative h-[380px] w-[380px]">
              <div
                className="absolute inset-[-40%] rounded-full animate-pulse-glow"
                style={{
                  background: "radial-gradient(circle, hsla(228, 100%, 62%, 0.12), transparent 60%)",
                  filter: "blur(40px)",
                }}
              />
              <GlowOrb className="h-full w-full" />
            </div>

            {/* Floating badges */}
            <motion.div
              animate={{ y: [-8, 8, -8] }}
              transition={{ duration: 5, repeat: Infinity, ease: "easeInOut" }}
              className="absolute right-[-30px] top-[20%]"
            >
              <div className="glass-card flex items-center gap-2.5 rounded-full px-4 py-2.5">
                <Shield size={14} className="text-accent" />
                <span className="text-xs font-medium text-foreground">AI Security</span>
              </div>
            </motion.div>

            <motion.div
              animate={{ y: [8, -8, 8] }}
              transition={{ duration: 6, repeat: Infinity, ease: "easeInOut" }}
              className="absolute bottom-[18%] left-[-40px]"
            >
              <div className="glass-card flex items-center gap-2.5 rounded-full px-4 py-2.5">
                <Zap size={14} className="text-primary" />
                <span className="text-xs font-medium text-foreground">Protocol Intelligence</span>
              </div>
            </motion.div>
          </motion.div>
        </div>

        {/* Scroll indicator */}
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ delay: 1.5 }}
          className="absolute bottom-8 left-1/2 -translate-x-1/2"
        >
          <motion.div
            animate={{ y: [0, 8, 0] }}
            transition={{ duration: 2, repeat: Infinity }}
            className="flex flex-col items-center gap-2"
          >
            <span className="text-[10px] uppercase tracking-widest text-muted-foreground">Scroll</span>
            <div className="h-8 w-[1px] bg-gradient-to-b from-muted-foreground/50 to-transparent" />
          </motion.div>
        </motion.div>
      </section>

      {/* ═══════════════ WHAT IS NOVAI ═══════════════ */}
      <section className="relative py-32">
        <div className="absolute inset-0 mesh-gradient pointer-events-none opacity-50" />
        {/* Extra nebula glows */}
        <div
          className="absolute pointer-events-none"
          style={{
            width: 600,
            height: 600,
            top: "20%",
            left: "5%",
            background: "radial-gradient(circle, hsla(228, 100%, 62%, 0.06), transparent 60%)",
            filter: "blur(60px)",
          }}
        />
        <div className="section-container relative z-10">
          <ScrollSection>
            <div className="text-center mb-20">
              <span className="pill-badge mb-6">Protocol Overview</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-6">
                What is <span className="gradient-text">NOVAInetwork</span>?
              </h2>
              <p className="max-w-2xl mx-auto text-muted-foreground leading-relaxed">
                A standalone Layer-1 blockchain built from first principles in Rust that integrates AI entities
                as first-class protocol primitives — not wrappers, not sidecars, but native protocol primitives with autonomous capabilities.
              </p>
            </div>
          </ScrollSection>

          <div className="grid gap-6 md:grid-cols-2">
            {FEATURES.map((feature, i) => (
              <ScrollSection key={feature.title} delay={i * 0.1}>
                <GlowCard className="h-full">
                  <div className="p-8">
                    <div
                      className="mb-5 flex h-12 w-12 items-center justify-center rounded-xl transition-colors duration-300"
                      style={{
                        background: "hsla(228, 100%, 62%, 0.08)",
                        border: "1px solid hsla(228, 100%, 62%, 0.15)",
                      }}
                    >
                      <feature.icon
                        size={22}
                        className="text-primary transition-colors group-hover:text-accent"
                      />
                    </div>
                    <h3 className="font-display text-xl font-semibold mb-3 text-foreground">
                      {feature.title}
                    </h3>
                    <p className="text-sm leading-relaxed text-muted-foreground">
                      {feature.description}
                    </p>
                  </div>
                </GlowCard>
              </ScrollSection>
            ))}
          </div>
        </div>
      </section>

      {/* ═══════════════ STATS ═══════════════ */}
      <section className="relative py-24">
        <div className="gradient-divider" />
        <div className="section-container relative z-10 py-20">
          <ScrollSection>
            <div className="text-center mb-16">
              <span className="pill-badge mb-6">Private Testnet Metrics</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
                Numbers that <span className="gradient-text">matter</span>
              </h2>
              <p className="text-muted-foreground">Clean-room engineering. Every line built from scratch.</p>
            </div>
          </ScrollSection>

          <div className="grid gap-8 sm:grid-cols-2 lg:grid-cols-4">
            {STATS.map((stat, i) => (
              <ScrollSection key={stat.label} delay={i * 0.15}>
                <div className="text-center">
                  <div className="font-display text-5xl font-bold gradient-text mb-3">
                    {stat.textOnly ? (
                      <motion.span
                        initial={{ opacity: 0, y: 10 }}
                        whileInView={{ opacity: 1, y: 0 }}
                        viewport={{ once: true }}
                        transition={{ duration: 0.5 }}
                        className="font-display"
                      >
                        {stat.textOnly}
                      </motion.span>
                    ) : (
                      <AnimatedCounter
                        target={stat.value!}
                        suffix={stat.suffix}
                        prefix={stat.prefix}
                      />
                    )}
                  </div>
                  <p className="text-sm text-muted-foreground">{stat.label}</p>
                </div>
              </ScrollSection>
            ))}
          </div>

          {/* Rust badge */}
          <ScrollSection delay={0.4}>
            <div className="mt-16 flex justify-center">
              <div className="glass-card rounded-2xl px-8 py-5 flex items-center gap-4">
                <Code2 size={24} className="text-accent" />
                <div>
                  <p className="font-display text-lg font-semibold text-foreground">Built from Scratch in Rust</p>
                  <p className="text-xs text-muted-foreground">No forks. No copied code. Pure clean-room engineering.</p>
                </div>
              </div>
            </div>
          </ScrollSection>
        </div>
        <div className="gradient-divider" />
      </section>

      {/* ═══════════════ ROADMAP ═══════════════ */}
      <section className="relative py-32">
        <div className="absolute inset-0 grid-bg-fine opacity-20 pointer-events-none" />
        <div
          className="absolute pointer-events-none"
          style={{
            width: 800,
            height: 800,
            top: "30%",
            right: "0%",
            background: "radial-gradient(circle, hsla(192, 95%, 68%, 0.05), transparent 60%)",
            filter: "blur(70px)",
          }}
        />
        <div className="section-container relative z-10">
          <ScrollSection>
            <div className="text-center mb-20">
              <span className="pill-badge mb-6">Roadmap</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
                The path <span className="gradient-text">forward</span>
              </h2>
              <p className="text-muted-foreground max-w-lg mx-auto">
                A phased approach to building the intelligent blockchain.
              </p>
            </div>
          </ScrollSection>

          <div className="grid gap-6 md:grid-cols-2 lg:grid-cols-4">
            {ROADMAP.map((phase, i) => (
              <ScrollSection key={phase.phase} delay={i * 0.1}>
                <div className="glass-card-hover glow-border rounded-2xl p-6 h-full relative overflow-hidden">
                  {phase.status === "current" && (
                    <div
                      className="absolute top-0 left-0 right-0 h-[2px]"
                      style={{
                        background: "linear-gradient(90deg, hsl(228, 100%, 62%), hsl(192, 95%, 68%))",
                      }}
                    />
                  )}
                  <div className="flex items-center gap-2 mb-4">
                    <span className="text-xs font-semibold uppercase tracking-wider text-accent">
                      {phase.phase}
                    </span>
                    {phase.status === "current" && (
                      <span className="relative flex h-2 w-2">
                        <span className="absolute inline-flex h-2 w-2 animate-ping rounded-full bg-accent opacity-75" />
                        <span className="relative inline-flex h-2 w-2 rounded-full bg-accent" />
                      </span>
                    )}
                  </div>
                  <h3 className="font-display text-lg font-semibold mb-4 text-foreground">
                    {phase.title}
                  </h3>
                  <ul className="space-y-2.5">
                    {phase.items.map((item) => (
                      <li key={item} className="flex items-start gap-2 text-sm text-muted-foreground">
                        <ChevronRight size={14} className="mt-0.5 flex-shrink-0 text-primary/60" />
                        {item}
                      </li>
                    ))}
                  </ul>
                </div>
              </ScrollSection>
            ))}
          </div>
        </div>
      </section>

      {/* ═══════════════ TOKENOMICS TEASER ═══════════════ */}
      <section className="relative py-24">
        <div className="gradient-divider" />
        <div className="section-container relative z-10 py-20">
          <ScrollSection>
            <div className="text-center">
              <div className="mx-auto mb-6 flex h-16 w-16 items-center justify-center rounded-2xl"
                style={{
                  background: "hsla(228, 100%, 62%, 0.08)",
                  border: "1px solid hsla(228, 100%, 62%, 0.15)",
                }}
              >
                <Coins size={28} className="text-primary" />
              </div>
              <span className="pill-badge mb-6">Tokenomics</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4 mt-6">
                Tokenomics <span className="gradient-text">coming soon</span>
              </h2>
              <p className="max-w-lg mx-auto text-muted-foreground leading-relaxed mb-8">
                The NOVAI token model is being carefully designed to align incentives across validators,
                AI agents, developers, and the broader community. Details will be published ahead of mainnet.
              </p>
              <Link to="/socials" className="btn-ghost no-underline">
                Stay Updated <ArrowRight size={16} />
              </Link>
            </div>
          </ScrollSection>
        </div>
        <div className="gradient-divider" />
      </section>

      {/* ═══════════════ COMMUNITY CTA ═══════════════ */}
      <section className="relative py-32">
        <div className="section-container relative z-10">
          <ScrollSection>
            <div className="glass-card rounded-3xl p-12 sm:p-16 text-center relative overflow-hidden">
              <div
                className="absolute inset-0 pointer-events-none"
                style={{
                  background: "radial-gradient(ellipse 60% 40% at 50% 50%, hsla(228, 100%, 62%, 0.06), transparent)",
                }}
              />

              <h2 className="font-display text-3xl font-bold sm:text-5xl mb-6 relative z-10">
                Join the <span className="gradient-text">revolution</span>
              </h2>
              <p className="text-muted-foreground max-w-lg mx-auto mb-10 relative z-10">
                Be part of the community building the first truly intelligent blockchain.
                Connect with builders, researchers, and visionaries.
              </p>
              <div className="flex flex-wrap justify-center gap-4 relative z-10">
                <Link to="/socials" className="btn-primary no-underline">
                  Join Community <ArrowRight size={16} />
                </Link>
                <Link to="/vision" className="btn-ghost no-underline">
                  Read Our Vision
                </Link>
              </div>
            </div>
          </ScrollSection>
        </div>
      </section>

      <Footer />
    </div>
  );
}
