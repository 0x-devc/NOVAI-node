import { useState, useEffect, useRef, FormEvent } from "react";
import { motion, useScroll, useTransform } from "framer-motion";
import { Link } from "react-router-dom";
import { ArrowRight, Mail } from "lucide-react";
import ScrollSection from "@/components/novai/ScrollSection";
import Footer from "@/components/novai/Footer";
import { AiHeroBackground } from "@/components/ui/ai-hero-background";

const TERMINAL_LINES = [
  "$ novai-node --chain testnet --mode validator",
  "INFO  Loading genesis configuration...",
  "INFO  Initializing BFT consensus engine",
  "INFO  AI security module: ACTIVE",
  "INFO  Sparse Merkle Tree state initialized",
  "INFO  Connected to 4 peer validators",
  "INFO  Block #2,500,001 finalized (0.42s)",
  "INFO  Block #2,500,002 finalized (0.38s)",
  "INFO  AI threat scan: no anomalies detected",
  "INFO  Block #2,500,003 finalized (0.41s)",
  "INFO  Network health: OPTIMAL",
];

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

export default function TestnetPage() {
  return (
    <div className="relative">
      {/* Hero with AI dot-grid background */}
      <section className="relative min-h-screen flex items-center overflow-hidden">
        <AiHeroBackground />
        {/* Overlay to blend into site theme */}
        <div className="absolute inset-0 pointer-events-none" style={{ background: "radial-gradient(ellipse 80% 60% at 50% 40%, transparent 30%, hsla(228, 30%, 4%, 0.85) 100%)" }} />

        <div className="section-container relative z-10 pt-28 pb-20">
          <div className="grid gap-16 lg:grid-cols-2 items-center">
            <motion.div initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.8 }}>
              <span className="pill-badge mb-8">Testnet</span>
              <h1 className="font-display text-4xl font-bold sm:text-5xl lg:text-6xl mb-6 leading-[1.1]">
                {"The NOVAInetwork Testnet is".split(" ").map((word, i) => (
                  <motion.span key={i} initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.2 + i * 0.08, ease: "easeOut" }} className="inline-block mr-[0.3em]">{word}</motion.span>
                ))}{" "}
                <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.6, ease: "easeOut" }} className="inline-block gradient-text">Coming Soon</motion.span>
              </h1>
              <p className="text-lg text-muted-foreground mb-8 leading-relaxed max-w-lg">We're preparing something special. The public testnet will let you experience AI-native blockchain infrastructure firsthand.</p>
              <div className="glass-card rounded-2xl px-6 py-5 flex items-center gap-4 mb-8 max-w-sm">
                <span className="relative flex h-3 w-3"><span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-75" /><span className="relative inline-flex h-3 w-3 rounded-full bg-accent" /></span>
                <div><p className="text-sm font-semibold text-foreground">Testnet Status</p><p className="text-xs text-accent">In Development</p></div>
              </div>
              <p className="text-sm text-muted-foreground mb-6">Join our community to be the first to know when it goes live.</p>
              <Link to="/socials" className="btn-primary no-underline">Join Community <ArrowRight size={16} /></Link>
            </motion.div>
            <motion.div initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.8, delay: 0.3 }}>
              <Terminal3D />
            </motion.div>
          </div>
        </div>
      </section>

      {/* What to expect */}
      <section className="relative py-24">
        <div className="gradient-divider" />
        <div className="section-container py-20">
          <ScrollSection><div className="text-center mb-16"><h2 className="font-display text-3xl font-bold sm:text-4xl mb-4">What to <span className="gradient-text">expect</span></h2><p className="text-muted-foreground max-w-lg mx-auto">The public testnet will showcase the core capabilities of the NOVAInetwork protocol.</p></div></ScrollSection>
          <div className="grid gap-6 sm:grid-cols-3 max-w-4xl mx-auto">
            {[{ title: "BFT Consensus", desc: "Experience HotStuff-inspired consensus in action" }, { title: "AI Agents", desc: "Interact with native AI protocol participants" }, { title: "Developer SDK", desc: "Build and deploy dApps on AI-native infrastructure" }].map((item, i) => (
              <ScrollSection key={item.title} delay={i * 0.1}><div className="glass-card-hover glow-border rounded-2xl p-6 text-center h-full"><h3 className="font-display text-lg font-semibold mb-2 text-foreground">{item.title}</h3><p className="text-sm text-muted-foreground">{item.desc}</p></div></ScrollSection>
            ))}
          </div>
        </div>
      </section>

      {/* How it works */}
      <section className="relative py-24">
        <div className="gradient-divider" />
        <div className="section-container py-20">
          <ScrollSection><div className="text-center mb-16"><h2 className="font-display text-3xl font-bold sm:text-4xl mb-4">How the testnet <span className="gradient-text">works</span></h2></div></ScrollSection>
          <div className="max-w-3xl mx-auto">
            {[{ step: "01", title: "Connect", desc: "Set up your node and connect to the NOVAI testnet. No special hardware required." }, { step: "02", title: "Validate", desc: "Participate in HotStuff BFT consensus. Propose and validate blocks alongside AI entities." }, { step: "03", title: "Interact", desc: "Submit transactions, deploy contracts, and interact with AI-native protocol primitives." }].map((item, i) => (
              <ScrollSection key={item.step} delay={i * 0.1}><div className="flex gap-6 sm:gap-8 mb-12 last:mb-0"><div className="flex-shrink-0"><span className="font-display text-5xl font-bold gradient-text opacity-30">{item.step}</span></div><div><h3 className="font-display text-xl font-semibold text-foreground mb-2">{item.title}</h3><p className="text-muted-foreground leading-relaxed">{item.desc}</p></div></div></ScrollSection>
            ))}
          </div>
        </div>
      </section>

      {/* Email signup */}
      <section className="relative py-24">
        <div className="gradient-divider" />
        <div className="section-container py-20">
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

      <Footer />
    </div>
  );
}
