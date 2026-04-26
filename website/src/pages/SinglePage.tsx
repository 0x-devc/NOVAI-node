import { useState, useEffect, useRef, FormEvent } from "react";
import { motion, useScroll, useTransform } from "framer-motion";
import {
  ArrowRight,
  Shield,
  Zap,
  Brain,
  Layers,
  Code2,
  ChevronRight,
  Coins,
  ExternalLink,
  FileText,
  Mail,
  Github,
} from "lucide-react";

import GlowOrb from "@/components/novai/GlowOrb";
import AnimatedCounter from "@/components/novai/AnimatedCounter";
import ScrollSection from "@/components/novai/ScrollSection";
import GlowCard from "@/components/novai/GlowCard";
import Footer from "@/components/novai/Footer";
import { FloatingPaths } from "@/components/ui/floating-paths";
import { CpuArchitecture } from "@/components/ui/cpu-architecture";
import { ContainerScroll } from "@/components/ui/container-scroll-animation";
import { TypewriterEffect } from "@/components/ui/typewriter-effect";
import { XIcon, DiscordIcon, TelegramIcon, GitHubIcon } from "@/components/novai/SocialIcons";

const EASE = [0.25, 0.46, 0.45, 0.94] as const;

const FEATURES = [
  {
    icon: Brain,
    title: "AI-Aware Consensus",
    description:
      "AI entity signals integrated directly into the consensus message format. The protocol carries intelligence as a first-class data type, enabling real-time monitoring, advisory threat detection, and adaptive network signaling.",
  },
  {
    icon: Shield,
    title: "Quantum-Aware Design",
    description:
      "Designed with future post-quantum migration in mind. As NIST standards mature, the architecture is positioned for transition to quantum-resistant primitives.",
  },
  {
    icon: Zap,
    title: "Self-Adjusting Protocol",
    description:
      "The network detects congestion patterns and flags suspicious transactions for deprioritisation, all within governance-approved bounds. Advisory AI signals guide real-time adaptation with built-in safety limits.",
  },
  {
    icon: Layers,
    title: "High Performance BFT",
    description:
      "HotStuff-inspired BFT consensus with Sparse Merkle Tree state management for fast, predictable, low-cost transactions.",
  },
];

const STATS: { value?: number; suffix?: string; label: string; prefix?: string; textOnly?: string }[] = [
  { textOnly: "Active", label: "Private Testnet Running" },
  { value: 4, suffix: "", label: "Active Validators", prefix: "" },
  { value: 4000, suffix: "+", label: "Tests Passing", prefix: "" },
  { value: 30, suffix: "M+", label: "Blocks Committed", prefix: "" },
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
    items: ["On-chain AI agents", "Autonomous protocol operations", "Governance expansion", "Tokenomics design"],
  },
  {
    phase: "Phase 4",
    title: "Mainnet",
    status: "future",
    items: ["Mainnet launch", "Tokenomics live", "Hackathons & ecosystem events", "Ecosystem growth & partnerships"],
  },
];

const PILLARS = [
  {
    title: "Self-Adjusting",
    description:
      "The protocol detects congestion patterns, identifies inefficiencies, and responds to emerging threats. It adjusts advisory parameters within governance-approved bounds to maintain peak performance. No committee votes. Safe limits enforced.",
  },
  {
    title: "AI-Driven Security",
    description:
      "AI entities monitor the network in real time, scoring transaction patterns for anomalies. Suspicious activity is flagged for deprioritisation (never censored) with full transparency for validator operators. Proactive defense built into the protocol's DNA.",
  },
  {
    title: "Quantum-Aware Design",
    description:
      "Designed with future post-quantum migration in mind. As NIST standards mature, the architecture is positioned for transition to quantum-resistant primitives. A forward-looking design consideration, not a bolted-on afterthought.",
  },
  {
    title: "High Performance BFT",
    description:
      "HotStuff-inspired BFT consensus with Sparse Merkle Tree state management. Deterministic transaction execution means predictable performance.",
  },
  {
    title: "Developer-First",
    description:
      "Purpose-built transaction types for AI entity management, value transfer, and autonomous agent operations. AI entities can originate transactions, hold balance, and store persistent state as independent protocol participants.",
  },
];

const SOCIALS = [
  { href: "https://github.com/0x-devc/NOVAI-node", title: "GitHub", description: "Full open source codebase - 65,000+ lines of Rust, 16 crates, Apache 2.0.", icon: GitHubIcon, followers: "Open Source", color: "210, 10%, 40%" },
  { href: "https://x.com/NOVAInetwork", title: "X (Twitter)", description: "Updates, announcements, and progress in public.", icon: XIcon, followers: "Growing", color: "228, 100%, 62%" },
  { href: "https://discord.gg/NTWr6x2dbM", title: "Discord", description: "Main community hub for ideas, feedback, and discussions.", icon: DiscordIcon, followers: "Active", color: "235, 86%, 65%" },
  { href: "https://t.me/+QoacVmowWNRkZjNk", title: "Telegram", description: "Updates and announcements.", icon: TelegramIcon, followers: "Live", color: "200, 90%, 55%" },
];

const DOCUMENTS = [
  {
    href: "https://github.com/0x-devc/NOVAI-node/blob/main/docs/tutorials/FIRST_AI_ENTITY.md",
    title: "Quick Start Tutorial",
    description: "Build your first AI entity in 10 minutes - keygen, faucet, register, publish a signal, query state.",
  },
  {
    href: "https://github.com/0x-devc/NOVAI-node/blob/main/docs/RPC_REFERENCE.md",
    title: "RPC Reference",
    description: "Every JSON-RPC endpoint with request and response shapes, error codes, and curl examples.",
  },
  {
    href: "https://github.com/0x-devc/NOVAI-node/blob/main/docs/ARCHITECTURE.md",
    title: "Architecture Deep Dive",
    description: "Crate-by-crate walkthrough with consensus and transaction-lifecycle flow diagrams.",
  },
  {
    href: "https://github.com/0x-devc/NOVAI-node/tree/main/sdk/novai-sdk-ts/examples/quick-start",
    title: "TypeScript SDK",
    description: "Connect, fund, transfer, and register an AI entity end-to-end from JavaScript.",
  },
  {
    href: "https://github.com/0x-devc/NOVAI-node/tree/main/sdk/novai-sdk/examples/quick-start",
    title: "Rust SDK",
    description: "The same flow as the TypeScript example, in idiomatic async Rust on tokio.",
  },
  {
    href: "https://dev.to/0xdevc/the-bug-that-silently-broke-my-entire-blockchain-how-a-single-function-rejected-trailing-bytes-4fij",
    title: "Blog: The Bug That Broke My Blockchain",
    description: "How a single function rejecting trailing bytes silently killed every block - a debugging story.",
  },
];

const TERMINAL_LINES = [
  "$ novai-node run --port 9000 --validator 0",
  "INFO  Loading genesis configuration...",
  "INFO  Initializing BFT consensus engine",
  "INFO  AI copilot module: ACTIVE",
  "INFO  Sparse Merkle Tree state initialized",
  "INFO  Connected to 4 peer validators",
  "INFO  Block #2,500,001 finalized (14ms)",
  "INFO  Block #2,500,002 finalized (12ms)",
  "INFO  Copilot advisory: no anomalies detected",
  "INFO  Block #2,500,003 finalized (15ms)",
  "INFO  Network health: OPTIMAL",
];

/* ─── Internal components ─── */

function TerminalAnimation() {
  const [lines, setLines] = useState<string[]>([]);
  const [currentLine, setCurrentLine] = useState(0);

  useEffect(() => {
    if (currentLine >= TERMINAL_LINES.length) {
      const timeout = setTimeout(() => { setLines([]); setCurrentLine(0); }, 3000);
      return () => clearTimeout(timeout);
    }
    const timeout = setTimeout(() => {
      setLines((prev) => [...prev, TERMINAL_LINES[currentLine]]);
      setCurrentLine((prev) => prev + 1);
    }, 600 + Math.random() * 400);
    return () => clearTimeout(timeout);
  }, [currentLine]);

  return (
    <div className="rounded-xl overflow-hidden text-left font-mono text-xs sm:text-sm" style={{ background: "hsla(228, 40%, 3%, 0.9)", border: "1px solid hsla(224, 20%, 18%, 0.6)" }}>
      <div className="flex items-center gap-2 px-4 py-2.5" style={{ borderBottom: "1px solid hsla(224, 20%, 18%, 0.4)" }}>
        <div className="flex gap-1.5">
          <div className="h-2.5 w-2.5 rounded-full" style={{ background: "hsl(0, 70%, 50%)" }} />
          <div className="h-2.5 w-2.5 rounded-full" style={{ background: "hsl(40, 70%, 50%)" }} />
          <div className="h-2.5 w-2.5 rounded-full" style={{ background: "hsl(120, 50%, 45%)" }} />
        </div>
        <span className="text-[10px] text-muted-foreground ml-2">novai-testnet</span>
      </div>
      <div className="p-4 h-[280px] overflow-hidden">
        {lines.map((line, i) => (
          <motion.div key={`${i}-${line}`} initial={{ opacity: 0, x: -5 }} animate={{ opacity: 1, x: 0 }} transition={{ duration: 0.2 }}
            className={`mb-1 ${line.startsWith("$") ? "text-accent" : line.includes("OPTIMAL") || line.includes("ACTIVE") ? "text-green-400" : line.includes("finalized") ? "text-primary/80" : "text-muted-foreground"}`}
          >{line}</motion.div>
        ))}
        {currentLine < TERMINAL_LINES.length && <span className="inline-block w-2 h-4 bg-accent animate-blink" />}
      </div>
    </div>
  );
}

function EmailSignup() {
  const [email, setEmail] = useState("");
  const [status, setStatus] = useState<"idle" | "loading" | "success" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault(); setErrorMsg("");
    const trimmed = email.trim();
    if (!trimmed || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(trimmed)) { setStatus("error"); setErrorMsg("Please enter a valid email address."); return; }
    const apiUrl = import.meta.env.VITE_WAITLIST_API_URL;
    if (!apiUrl) { setStatus("error"); setErrorMsg("Signup is not available right now."); return; }
    setStatus("loading");
    try {
      const res = await fetch(apiUrl, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: trimmed }) });
      if (!res.ok) throw new Error("fail"); setStatus("success");
    } catch { setStatus("error"); setErrorMsg("Something went wrong. Please try again."); }
  };

  if (status === "success") return (
    <motion.div initial={{ opacity: 0, scale: 0.95 }} animate={{ opacity: 1, scale: 1 }} className="glass-card rounded-2xl px-6 py-5 inline-flex items-center gap-3">
      <span className="relative flex h-3 w-3"><span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-75" /><span className="relative inline-flex h-3 w-3 rounded-full bg-accent" /></span>
      <span className="text-sm font-semibold text-foreground">You're on the list!</span>
    </motion.div>
  );

  return (
    <form onSubmit={handleSubmit} className="flex flex-col items-center gap-3">
      <div className="flex w-full max-w-md gap-3">
        <input type="email" placeholder="you@example.com" value={email} onChange={(e) => { setEmail(e.target.value); if (status === "error") setStatus("idle"); }}
          className="flex-1 rounded-xl px-4 py-3 text-sm text-foreground placeholder:text-muted-foreground/60 outline-none focus:ring-2 focus:ring-primary/50 transition-shadow"
          style={{ background: "hsla(224, 28%, 10%, 0.6)", border: "1px solid hsla(224, 20%, 25%, 0.6)" }} />
        <button type="submit" disabled={status === "loading"} className="btn-primary whitespace-nowrap disabled:opacity-50">{status === "loading" ? "Sending..." : "Notify Me"}</button>
      </div>
      {status === "error" && errorMsg && <p className="text-xs text-destructive">{errorMsg}</p>}
    </form>
  );
}

function Terminal3D() {
  const ref = useRef<HTMLDivElement>(null);
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start end", "end start"] });
  const rotateX = useTransform(scrollYProgress, [0, 0.5], [12, 0]);
  const scale = useTransform(scrollYProgress, [0, 0.5], [0.95, 1]);

  return (
    <div ref={ref} style={{ perspective: 1000 }}>
      <motion.div style={{ rotateX, scale, transformStyle: "preserve-3d" }}>
        <div style={{ boxShadow: "0 25px 60px -12px rgba(0, 0, 0, 0.5), 0 0 40px -8px hsla(228, 100%, 62%, 0.08)" }} className="rounded-xl">
          <TerminalAnimation />
        </div>
      </motion.div>
    </div>
  );
}

/* ─── Roadmap sticky-scroll component ─── */
function StickyRoadmap() {
  const containerRef = useRef<HTMLDivElement>(null);
  const { scrollYProgress } = useScroll({ target: containerRef });
  const total = ROADMAP.length;

  // Active index: 0-3 based on scroll within this container
  const activeRaw = useTransform(scrollYProgress, [0, 1], [0, total]);

  return (
    <section ref={containerRef} className="relative" style={{ height: `${total * 100}vh` }}>
      <div className="sticky top-0 h-screen flex items-center overflow-hidden">
        <div className="section-container relative z-10 w-full">
          <div className="text-center mb-12">
            <span className="pill-badge mb-6">Roadmap</span>
            <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">The path <span className="gradient-text">forward</span></h2>
            <p className="text-muted-foreground max-w-lg mx-auto">A phased approach to building the intelligent blockchain.</p>
          </div>

          {/* Phase indicator dots */}
          <div className="flex justify-center gap-4 mb-10">
            {ROADMAP.map((phase, i) => {
              const dotOpacity = useTransform(activeRaw, (v) => (Math.floor(v) === i || (i === total - 1 && v >= total - 0.5)) ? 1 : 0.3);
              const dotScale = useTransform(activeRaw, (v) => (Math.floor(v) === i || (i === total - 1 && v >= total - 0.5)) ? 1.4 : 1);
              return (
                <motion.div key={phase.phase} style={{ opacity: dotOpacity, scale: dotScale }} className="flex flex-col items-center gap-1.5">
                  <motion.div
                    className="w-3 h-3 rounded-full"
                    style={{
                      background: useTransform(activeRaw, (v) =>
                        (Math.floor(v) === i || (i === total - 1 && v >= total - 0.5))
                          ? "hsl(228, 100%, 62%)"
                          : "hsla(215, 20%, 55%, 0.3)"
                      ),
                      boxShadow: useTransform(activeRaw, (v) =>
                        (Math.floor(v) === i || (i === total - 1 && v >= total - 0.5))
                          ? "0 0 12px hsla(228, 100%, 62%, 0.5)"
                          : "none"
                      ),
                    }}
                  />
                  <span className="text-[10px] text-muted-foreground whitespace-nowrap">{phase.phase}</span>
                </motion.div>
              );
            })}
          </div>

          {/* Phase cards - only one visible at a time */}
          <div className="max-w-2xl mx-auto relative h-[300px] sm:h-[320px]">
            {ROADMAP.map((phase, i) => {
              const start = i / total;
              const end = (i + 1) / total;
              const cardOpacity = useTransform(scrollYProgress, [
                Math.max(0, start - 0.03), start + 0.01, end - 0.04, Math.min(1, end - 0.01)
              ], [0, 1, 1, 0]);
              const cardY = useTransform(scrollYProgress, [Math.max(0, start - 0.03), start + 0.01], [30, 0]);

              return (
                <motion.div key={phase.phase} style={{ opacity: cardOpacity, y: cardY }} className="absolute inset-0">
                  <div className="glass-card glow-border rounded-2xl p-8 sm:p-10 relative overflow-hidden h-full">
                    {phase.status === "current" && <div className="absolute top-0 left-0 right-0 h-[2px]" style={{ background: "linear-gradient(90deg, hsl(228, 100%, 62%), hsl(192, 95%, 68%))" }} />}
                    <div className="flex items-center gap-3 mb-4">
                      <span className="font-display text-5xl font-bold gradient-text opacity-25">{String(i + 1).padStart(2, "0")}</span>
                      <div>
                        <span className="text-xs font-semibold uppercase tracking-wider text-accent">{phase.phase}</span>
                        {phase.status === "current" && (
                          <span className="relative flex h-2 w-2 inline-flex ml-2">
                            <span className="absolute inline-flex h-2 w-2 animate-ping rounded-full bg-accent opacity-75" />
                            <span className="relative inline-flex h-2 w-2 rounded-full bg-accent" />
                          </span>
                        )}
                      </div>
                    </div>
                    <h3 className="font-display text-2xl sm:text-3xl font-semibold mb-6 text-foreground">{phase.title}</h3>
                    <ul className="space-y-3">
                      {phase.items.map((item) => (
                        <li key={item} className="flex items-start gap-2 text-sm text-muted-foreground">
                          <ChevronRight size={14} className="mt-0.5 flex-shrink-0 text-primary/60" />{item}
                        </li>
                      ))}
                    </ul>
                  </div>
                </motion.div>
              );
            })}
          </div>
        </div>
      </div>
    </section>
  );
}

/* ─── Design Pillars sticky-scroll ─── */
function StickyPillars() {
  const containerRef = useRef<HTMLDivElement>(null);
  const { scrollYProgress } = useScroll({ target: containerRef });
  const total = PILLARS.length;
  const activeRaw = useTransform(scrollYProgress, [0, 1], [0, total]);

  return (
    <section ref={containerRef} className="relative" style={{ height: `${total * 80}vh` }}>
      <div className="sticky top-0 h-screen flex items-center overflow-hidden">
        <div className="section-container relative z-10 w-full">
          <div className="grid lg:grid-cols-2 gap-12 items-center">
            {/* Left: heading + step indicator */}
            <div>
              <span className="pill-badge mb-6">Design Principles</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-6">Built to <span className="gradient-text">evolve</span></h2>
              {/* Step dots */}
              <div className="flex gap-2 mb-6">
                {PILLARS.map((_, i) => {
                  const dotOpacity = useTransform(activeRaw, (v) => (Math.floor(v) === i || (i === total - 1 && v >= total - 0.5)) ? 1 : 0.25);
                  return (
                    <motion.div key={i} style={{ opacity: dotOpacity }} className="w-8 h-1 rounded-full bg-primary" />
                  );
                })}
              </div>
              {/* Active number */}
              <div className="relative h-[60px]">
                {PILLARS.map((_, i) => {
                  const start = i / total;
                  const end = (i + 1) / total;
                  const numOpacity = useTransform(scrollYProgress, [Math.max(0, start - 0.02), start + 0.01, end - 0.03, Math.min(1, end - 0.01)], [0, 1, 1, 0]);
                  return (
                    <motion.span key={i} style={{ opacity: numOpacity }} className="absolute font-display text-6xl font-bold gradient-text opacity-30">{String(i + 1).padStart(2, "0")}</motion.span>
                  );
                })}
              </div>
            </div>

            {/* Right: pillar card that swaps */}
            <div className="relative h-[320px] sm:h-[350px]">
              {PILLARS.map((pillar, i) => {
                const start = i / total;
                const end = (i + 1) / total;
                const cardOpacity = useTransform(scrollYProgress, [Math.max(0, start - 0.03), start + 0.01, end - 0.04, Math.min(1, end - 0.01)], [0, 1, 1, 0]);
                const cardY = useTransform(scrollYProgress, [Math.max(0, start - 0.03), start + 0.01], [25, 0]);
                return (
                  <motion.div key={pillar.title} style={{ opacity: cardOpacity, y: cardY }} className="absolute inset-0">
                    <div className="glass-card glow-border rounded-2xl p-10 sm:p-12 h-full overflow-hidden">
                      <h3 className="font-display text-3xl font-semibold mb-5 text-foreground">{pillar.title}</h3>
                      <p className="text-lg text-muted-foreground leading-relaxed">{pillar.description}</p>
                    </div>
                  </motion.div>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

/* ─── Main single-page component ─── */

export default function SinglePage() {
  return (
    <div className="relative">

      {/* Global backgrounds - fixed, uniform across entire page */}
      <div className="fixed inset-0 pointer-events-none z-0">
        <div className="absolute inset-0 mesh-gradient opacity-70" />
        <div className="absolute inset-0 grid-bg-fine opacity-10" />
      </div>
      <div className="fixed inset-0 pointer-events-none z-0 opacity-40">
        <FloatingPaths position={1} />
        <FloatingPaths position={-1} />
      </div>

      {/* ═══════════════ HOME - HERO ═══════════════ */}
      <section id="home" className="relative min-h-screen flex items-center overflow-hidden">

        {/* Nebula glows */}

        <div className="section-container relative z-10 flex w-full flex-col items-center gap-16 pt-24 pb-20 lg:flex-row lg:justify-between">
          <motion.div initial={{ opacity: 0, y: 40 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.8, ease: EASE }} className="max-w-2xl">
            <div className="pill-badge mb-8">
              <span className="relative flex h-2 w-2">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-75" />
                <span className="relative inline-flex h-2 w-2 rounded-full bg-accent" />
              </span>
              Now Open Source
            </div>

            <h1 className="font-display font-bold leading-[1.1] tracking-tight mb-6">
              <span className="block gradient-text text-5xl sm:text-6xl lg:text-7xl">NOVAInetwork</span>
              <span className="block text-foreground text-3xl sm:text-4xl lg:text-5xl mt-2">
                {"The AI-Integrated Layer 1 Blockchain".split(" ").map((word, i) => (
                  <motion.span key={i} initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.3 + i * 0.08, ease: "easeOut" }} className="inline-block mr-[0.3em]">{word}</motion.span>
                ))}
              </span>
              <motion.span className="block text-xl sm:text-2xl mt-3 italic" style={{ color: "hsl(192, 95%, 68%)" }} initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ duration: 0.6, delay: 0.9 }}>
                The intelligent network is awakening.
              </motion.span>
            </h1>

            <p className="text-lg text-muted-foreground leading-relaxed mb-4 max-w-xl">NOVAInetwork is a standalone Layer-1 blockchain with AI embedded directly into the protocol layer, not bolted on as an afterthought.</p>
            <p className="text-sm text-muted-foreground mb-8 max-w-lg">Built from scratch in Rust. HotStuff BFT consensus. Deterministic execution. AI agents as first-class protocol citizens.</p>

            <div className="flex flex-wrap gap-4 mb-8">
              <a href="#documents" className="btn-primary no-underline">Documentation <ArrowRight size={16} /></a>
              <a
                href="https://github.com/0x-devc/NOVAI-node"
                target="_blank"
                rel="noopener noreferrer"
                className="btn-ghost no-underline"
              >
                <Github size={16} /> View Source
              </a>
              <a href="#socials" className="btn-ghost no-underline">Join Community</a>
            </div>
            <p className="text-xs text-muted-foreground/60">Status: Pre-mainnet · Open source · Private testnet live · Public testnet coming soon · Built from scratch in Rust</p>
          </motion.div>

          <motion.div initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} transition={{ duration: 1.2, delay: 0.3, ease: EASE }} className="relative flex-shrink-0 hidden lg:block">
            <div className="relative h-[380px] w-[380px]">
              <div className="absolute inset-[-40%] rounded-full animate-pulse-glow" style={{ background: "radial-gradient(circle, hsla(228, 100%, 62%, 0.12), transparent 60%)", filter: "blur(40px)" }} />
              <GlowOrb className="h-full w-full" />
            </div>
            <motion.div animate={{ y: [-8, 8, -8] }} transition={{ duration: 5, repeat: Infinity, ease: "easeInOut" }} className="absolute right-[-30px] top-[20%]">
              <div className="glass-card flex items-center gap-2.5 rounded-full px-4 py-2.5"><Shield size={14} className="text-accent" /><span className="text-xs font-medium text-foreground">AI Security</span></div>
            </motion.div>
            <motion.div animate={{ y: [8, -8, 8] }} transition={{ duration: 6, repeat: Infinity, ease: "easeInOut" }} className="absolute bottom-[18%] left-[-40px]">
              <div className="glass-card flex items-center gap-2.5 rounded-full px-4 py-2.5"><Zap size={14} className="text-primary" /><span className="text-xs font-medium text-foreground">Protocol Intelligence</span></div>
            </motion.div>
          </motion.div>
        </div>

        <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ delay: 1.5 }} className="absolute bottom-8 left-1/2 -translate-x-1/2">
          <motion.div animate={{ y: [0, 8, 0] }} transition={{ duration: 2, repeat: Infinity }} className="flex flex-col items-center gap-2">
            <span className="text-[10px] uppercase tracking-widest text-muted-foreground">Scroll</span>
            <div className="h-8 w-[1px] bg-gradient-to-b from-muted-foreground/50 to-transparent" />
          </motion.div>
        </motion.div>
      </section>

      {/* ═══════════════ GRADIENT TRANSITION ═══════════════ */}
      <div className="gradient-divider" />

      {/* ═══════════════ VISION ═══════════════ */}
      <section id="vision" className="relative min-h-[70vh] flex items-center overflow-hidden">

        {/* FloatingPaths now global - removed from here */}

        <div className="section-container relative z-10 pt-28 pb-20">
          <motion.div
            initial={{ opacity: 0, y: 30 }}
            whileInView={{ opacity: 1, y: 0 }}
            viewport={{ once: true }}
            transition={{ duration: 0.8 }}
            className="max-w-3xl"
          >
            <span className="pill-badge mb-8">Our Vision</span>

            {/* Typewriter heading */}
            <div className="mb-8">
              <TypewriterEffect
                className="font-display text-5xl font-bold leading-[1.1] sm:text-6xl lg:text-7xl text-left"
                words={[
                  { text: "Intelligence" },
                  { text: "is" },
                  { text: "not" },
                  { text: "a" },
                  { text: "feature.", className: "gradient-text" },
                  { text: "It's" },
                  { text: "the" },
                  { text: "foundation.", className: "gradient-text" },
                ]}
                cursorClassName="bg-accent h-8 lg:h-12"
              />
            </div>

            <motion.p
              className="text-xl text-muted-foreground leading-relaxed max-w-2xl"
              initial={{ opacity: 0 }}
              whileInView={{ opacity: 1 }}
              viewport={{ once: true }}
              transition={{ duration: 0.6, delay: 1.2 }}
            >
              NOVAInetwork is building toward a mainstream, AI-integrated Layer-1 blockchain.
              Designed for real-world scale, high performance, and a developer-first experience.
              AI entities operate as native protocol primitives: monitoring and signaling within governance-approved bounds.
            </motion.p>
          </motion.div>
        </div>
      </section>

      {/* Vision - Pull Quote */}
      <section className="relative py-20">
        <div className="gradient-divider" />
        <div className="section-container py-20">
          <ScrollSection>
            <blockquote className="text-center">
              <p className="font-display text-3xl sm:text-4xl font-light italic text-foreground/80 leading-relaxed max-w-3xl mx-auto">
                &ldquo;The goal is long-term, mainstream utility across applications,
                infrastructure, and{" "}
                <span className="gradient-text font-medium not-italic">real-world systems.</span>&rdquo;
              </p>
            </blockquote>
          </ScrollSection>
        </div>
        <div className="gradient-divider" />
      </section>

      {/* Vision - Pillars (sticky scroll) */}
      <StickyPillars />

      {/* ═══════════════ WHAT IS NOVAI ═══════════════ */}
      <section className="relative py-32">
        <div className="section-container relative z-10">
          <ScrollSection>
            <div className="text-center mb-20">
              <span className="pill-badge mb-6">Protocol Overview</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-6">What is <span className="gradient-text">NOVAInetwork</span>?</h2>
              <p className="max-w-2xl mx-auto text-muted-foreground leading-relaxed">
                A standalone Layer-1 blockchain built from first principles in Rust that integrates AI entities
                as first-class protocol primitives. Not wrappers, not sidecars, but native protocol primitives with autonomous capabilities.
              </p>
            </div>
          </ScrollSection>

          <div className="grid gap-6 md:grid-cols-2">
            {FEATURES.map((feature, i) => (
              <ScrollSection key={feature.title} delay={i * 0.1}>
                <GlowCard className="h-full">
                  <div className="p-8">
                    <div className="mb-5 flex h-12 w-12 items-center justify-center rounded-xl transition-colors duration-300" style={{ background: "hsla(228, 100%, 62%, 0.08)", border: "1px solid hsla(228, 100%, 62%, 0.15)" }}>
                      <feature.icon size={22} className="text-primary transition-colors group-hover:text-accent" />
                    </div>
                    <h3 className="font-display text-xl font-semibold mb-3 text-foreground">{feature.title}</h3>
                    <p className="text-sm leading-relaxed text-muted-foreground">{feature.description}</p>
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
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">Numbers that <span className="gradient-text">matter</span></h2>
              <p className="text-muted-foreground">Clean-room engineering. Every line built from scratch.</p>
            </div>
          </ScrollSection>
          <div className="grid gap-8 sm:grid-cols-2 lg:grid-cols-4">
            {STATS.map((stat, i) => (
              <ScrollSection key={stat.label} delay={i * 0.15}>
                <div className="text-center">
                  <div className="font-display text-5xl font-bold gradient-text mb-3">
                    {stat.textOnly ? (
                      <motion.span initial={{ opacity: 0, y: 10 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }} transition={{ duration: 0.5 }} className="font-display">{stat.textOnly}</motion.span>
                    ) : (
                      <AnimatedCounter target={stat.value!} suffix={stat.suffix} prefix={stat.prefix} />
                    )}
                  </div>
                  <p className="text-sm text-muted-foreground">{stat.label}</p>
                </div>
              </ScrollSection>
            ))}
          </div>
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
      <StickyRoadmap />

      {/* ═══════════════ TESTNET ═══════════════ */}
      <div id="testnet" className="relative">
        {/* Testnet Hero */}
        <section className="relative min-h-screen flex items-center overflow-hidden">
            <div className="absolute inset-0 grid-bg opacity-20 pointer-events-none" />

          <div className="section-container relative z-10 pt-28 pb-20">
            <div className="grid gap-16 lg:grid-cols-2 items-center">
              <motion.div initial={{ opacity: 0, y: 30 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }} transition={{ duration: 0.8 }}>
                <span className="pill-badge mb-8">Testnet</span>
                <h2 className="font-display text-4xl font-bold sm:text-5xl lg:text-6xl mb-6 leading-[1.1]">
                  {"The NOVAInetwork Testnet is".split(" ").map((word, i) => (
                    <motion.span key={i} initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} whileInView={{ opacity: 1, y: 0, filter: "blur(0px)" }} viewport={{ once: true }} transition={{ duration: 0.4, delay: 0.2 + i * 0.08, ease: "easeOut" }} className="inline-block mr-[0.3em]">{word}</motion.span>
                  ))}{" "}
                  <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} whileInView={{ opacity: 1, y: 0, filter: "blur(0px)" }} viewport={{ once: true }} transition={{ duration: 0.4, delay: 0.6, ease: "easeOut" }} className="inline-block gradient-text">Coming Soon</motion.span>
                </h2>
                <p className="text-lg text-muted-foreground mb-8 leading-relaxed max-w-lg">We're preparing something special. The public testnet will let you experience AI-native blockchain infrastructure firsthand.</p>
                <div className="glass-card rounded-2xl px-6 py-5 flex items-center gap-4 mb-8 max-w-sm">
                  <span className="relative flex h-3 w-3"><span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-75" /><span className="relative inline-flex h-3 w-3 rounded-full bg-accent" /></span>
                  <div><p className="text-sm font-semibold text-foreground">Testnet Status</p><p className="text-xs text-accent">In Development</p></div>
                </div>
                <p className="text-sm text-muted-foreground mb-6">Join our community to be the first to know when it goes live.</p>
                <a href="#socials" className="btn-primary no-underline">Join Community <ArrowRight size={16} /></a>
              </motion.div>
              <motion.div initial={{ opacity: 0, y: 30 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }} transition={{ duration: 0.8, delay: 0.3 }}>
                <Terminal3D />
              </motion.div>
            </div>
          </div>
        </section>

        {/* Testnet - What to expect */}
        <section className="relative py-24">
          <div className="gradient-divider" />
          <div className="section-container py-20 relative z-10">
            <ScrollSection><div className="text-center mb-16"><h2 className="font-display text-3xl font-bold sm:text-4xl mb-4">What to <span className="gradient-text">expect</span></h2><p className="text-muted-foreground max-w-lg mx-auto">The public testnet will showcase the core capabilities of the NOVAInetwork protocol.</p></div></ScrollSection>
            <div className="grid gap-6 sm:grid-cols-3 max-w-4xl mx-auto">
              {[{ title: "BFT Consensus", desc: "Experience HotStuff-inspired consensus in action" }, { title: "AI Agents", desc: "Interact with native AI protocol participants" }, { title: "Developer SDK", desc: "Build applications using the NOVAI SDK and native transaction types" }].map((item, i) => (
                <ScrollSection key={item.title} delay={i * 0.1}><div className="glass-card-hover glow-border rounded-2xl p-8 sm:p-10 text-center h-full"><h3 className="font-display text-xl sm:text-2xl font-semibold mb-3 text-foreground">{item.title}</h3><p className="text-base text-muted-foreground leading-relaxed">{item.desc}</p></div></ScrollSection>
              ))}
            </div>
          </div>
        </section>

        {/* Testnet - CPU Architecture 3D Scroll */}
        <ContainerScroll
          titleComponent={
            <>
              <h2 className="font-display text-3xl font-bold sm:text-4xl md:text-5xl mb-4 text-foreground">
                How the testnet <span className="gradient-text">works</span>
              </h2>
              <p className="text-muted-foreground max-w-2xl mx-auto text-base sm:text-lg">
                NOVAI protocol architecture, built from scratch in Rust.
              </p>
            </>
          }
        >
          <div className="h-full w-full flex flex-col items-center justify-center p-4 sm:p-8">
            <CpuArchitecture className="w-full max-w-2xl h-auto" />
          </div>
        </ContainerScroll>

        <div className="section-container relative z-10 -mt-40 pb-16">
          <div className="grid grid-cols-3 gap-4 max-w-2xl mx-auto">
            {[
              { step: "01", title: "Connect", desc: "Set up your node and connect to the NOVAI testnet." },
              { step: "02", title: "Validate", desc: "Participate in HotStuff BFT consensus alongside AI entities." },
              { step: "03", title: "Interact", desc: "Submit transactions and interact with AI-native protocol primitives." },
            ].map((item) => (
              <div key={item.step} className="text-center p-5 sm:p-6 rounded-xl" style={{ background: "hsla(224, 28%, 12%, 0.6)", border: "1px solid hsla(224, 20%, 25%, 0.3)" }}>
                <p className="text-sm font-bold gradient-text mb-2">{item.step}</p>
                <p className="text-lg font-semibold text-foreground mb-2">{item.title}</p>
                <p className="text-sm text-muted-foreground leading-relaxed">{item.desc}</p>
              </div>
            ))}
          </div>
        </div>

        {/* Testnet - Email signup */}
        <section className="relative py-24">
          <div className="gradient-divider" />
          <div className="section-container py-20 relative z-10">
            <ScrollSection>
              <div className="max-w-xl mx-auto text-center">
                <div className="mx-auto mb-6 flex h-14 w-14 items-center justify-center rounded-xl" style={{ background: "hsla(228, 100%, 62%, 0.08)", border: "1px solid hsla(228, 100%, 62%, 0.15)" }}><Mail size={24} className="text-primary" /></div>
                <h2 className="font-display text-3xl font-bold sm:text-4xl mb-4">Get notified when the public testnet goes{" "}<span className="gradient-text">live</span></h2>
                <p className="text-muted-foreground mb-8">Drop your email and we'll let you know the moment it launches.</p>
                <EmailSignup />
              </div>
            </ScrollSection>
          </div>
        </section>
      </div>

      {/* ═══════════════ SOCIALS ═══════════════ */}
      <section id="socials" className="relative min-h-[60vh] flex items-center overflow-hidden">
        <div className="section-container relative z-10 pt-28 pb-12">
          <motion.div initial={{ opacity: 0, y: 30 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }} transition={{ duration: 0.8, ease: EASE }} className="text-center max-w-2xl mx-auto">
            <span className="pill-badge mb-8">Community</span>
            <h2 className="font-display text-5xl font-bold sm:text-6xl mb-6">
              {"Connect with".split(" ").map((word, i) => (
                <motion.span key={i} initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} whileInView={{ opacity: 1, y: 0, filter: "blur(0px)" }} viewport={{ once: true }} transition={{ duration: 0.4, delay: 0.2 + i * 0.08, ease: "easeOut" }} className="inline-block mr-[0.3em]">{word}</motion.span>
              ))}
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} whileInView={{ opacity: 1, y: 0, filter: "blur(0px)" }} viewport={{ once: true }} transition={{ duration: 0.4, delay: 0.36, ease: "easeOut" }} className="inline-block gradient-text">NOVAInetwork</motion.span>
            </h2>
            <p className="text-lg text-muted-foreground">Join the community building the future of intelligent blockchain infrastructure.</p>
          </motion.div>
        </div>
      </section>

      <section className="relative pb-24">
        <div className="section-container relative z-10 max-w-2xl mx-auto">
          <div className="flex flex-col gap-5">
            {SOCIALS.map((social, i) => (
              <ScrollSection key={social.title} delay={i * 0.1}>
                <motion.a href={social.href} target="_blank" rel="noopener noreferrer" className="glass-card-hover glow-border rounded-2xl p-6 flex items-center justify-between group no-underline transition-all block" whileHover={{ y: -4 }} transition={{ duration: 0.3 }}>
                  <div className="flex items-center gap-5">
                    <div className="flex h-14 w-14 items-center justify-center rounded-xl transition-all duration-300 group-hover:scale-110" style={{ background: `hsla(${social.color}, 0.1)`, border: `1px solid hsla(${social.color}, 0.2)` }}>
                      <social.icon size={24} />
                    </div>
                    <div>
                      <h3 className="font-display text-base font-semibold text-foreground mb-0.5">{social.title}</h3>
                      <p className="text-sm text-muted-foreground">{social.description}</p>
                    </div>
                  </div>
                  <div className="flex items-center gap-3">
                    <span className="hidden sm:inline-block pill-badge text-[10px]">{social.followers}</span>
                    <ExternalLink size={16} className="text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5 group-hover:text-foreground" />
                  </div>
                </motion.a>
              </ScrollSection>
            ))}
          </div>
        </div>
      </section>

      {/* ═══════════════ DOCUMENTS ═══════════════ */}
      <section id="documents" className="relative py-20">

        <div className="section-container relative z-10 pt-16 pb-8 text-center">
          <motion.div initial={{ opacity: 0, y: 30 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true }} transition={{ duration: 0.8 }}>
            <span className="pill-badge mb-8"><FileText size={12} /> Documentation</span>
            <h2 className="font-display text-5xl font-bold sm:text-6xl mb-6">
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} whileInView={{ opacity: 1, y: 0, filter: "blur(0px)" }} viewport={{ once: true }} transition={{ duration: 0.4, delay: 0.2, ease: "easeOut" }} className="inline-block mr-[0.3em]">Documents</motion.span>{" "}
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} whileInView={{ opacity: 1, y: 0, filter: "blur(0px)" }} viewport={{ once: true }} transition={{ duration: 0.4, delay: 0.36, ease: "easeOut" }} className="inline-block gradient-text">&amp; Resources</motion.span>
            </h2>
            <p className="text-lg text-muted-foreground max-w-2xl mx-auto">
              Tutorials, references, SDK examples, and engineering write-ups. All open source on GitHub.
            </p>
          </motion.div>
        </div>

        {/* Document grid */}
        <div className="section-container relative z-10 max-w-5xl mx-auto pb-8">
          <div className="grid gap-5 md:grid-cols-2">
            {DOCUMENTS.map((doc, i) => (
              <ScrollSection key={doc.title} delay={i * 0.08}>
                <motion.a
                  href={doc.href}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="glass-card-hover glow-border rounded-2xl p-6 sm:p-7 block group no-underline relative overflow-hidden h-full"
                  whileHover={{ y: -4 }}
                  transition={{ duration: 0.3 }}
                >
                  <div className="absolute top-0 left-0 right-0 h-[2px]" style={{ background: "linear-gradient(90deg, hsl(228, 100%, 62%), hsl(192, 95%, 68%))" }} />
                  <div className="flex items-start justify-between gap-4 mb-3">
                    <h3 className="font-display text-lg font-semibold text-foreground leading-snug">{doc.title}</h3>
                    <ExternalLink size={16} className="text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5 group-hover:text-foreground flex-shrink-0 mt-1" />
                  </div>
                  <p className="text-sm text-muted-foreground leading-relaxed">{doc.description}</p>
                </motion.a>
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
              <div className="mx-auto mb-6 flex h-16 w-16 items-center justify-center rounded-2xl" style={{ background: "hsla(228, 100%, 62%, 0.08)", border: "1px solid hsla(228, 100%, 62%, 0.15)" }}><Coins size={28} className="text-primary" /></div>
              <span className="pill-badge mb-6">Tokenomics</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4 mt-6">Tokenomics <span className="gradient-text">coming soon</span></h2>
              <p className="max-w-lg mx-auto text-muted-foreground leading-relaxed mb-8">The NOVAI token model is being carefully designed to align incentives across validators, AI agents, developers, and the broader community.</p>
              <a href="#socials" className="btn-ghost no-underline">Stay Updated <ArrowRight size={16} /></a>
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
              <div className="absolute inset-0 pointer-events-none" style={{ background: "radial-gradient(ellipse 60% 40% at 50% 50%, hsla(228, 100%, 62%, 0.06), transparent)" }} />
              <h2 className="font-display text-3xl font-bold sm:text-5xl mb-6 relative z-10">Join the <span className="gradient-text">revolution</span></h2>
              <p className="text-muted-foreground max-w-lg mx-auto mb-10 relative z-10">Be part of the community building the first truly intelligent blockchain. Connect with builders, researchers, and visionaries.</p>
              <div className="flex flex-wrap justify-center gap-4 relative z-10">
                <a href="#socials" className="btn-primary no-underline">Join Community <ArrowRight size={16} /></a>
                <a href="#vision" className="btn-ghost no-underline">Read Our Vision</a>
              </div>
            </div>
          </ScrollSection>
        </div>
      </section>

      <Footer />
    </div>
  );
}
