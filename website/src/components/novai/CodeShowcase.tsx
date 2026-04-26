import { useState, useEffect, useRef } from "react";
import { motion, AnimatePresence, useInView } from "framer-motion";
import { Copy, Check } from "lucide-react";
import ScrollSection from "./ScrollSection";

type CodeLine = { text: string; color: string };

const TABS: { label: string; lines: CodeLine[] }[] = [
  {
    label: "Run a Node",
    lines: [
      { text: "$ novai-node \\", color: "text-accent" },
      { text: "    --chain mainnet \\", color: "text-foreground/80" },
      { text: "    --mode validator \\", color: "text-foreground/80" },
      { text: "    --ai-module enabled", color: "text-green-400" },
      { text: "", color: "" },
      { text: "INFO  Consensus engine initialized", color: "text-muted-foreground" },
      { text: "INFO  AI security module: ACTIVE", color: "text-green-400" },
      { text: "INFO  Sparse Merkle Tree state loaded", color: "text-muted-foreground" },
      { text: "INFO  Connected to 4 peer validators", color: "text-muted-foreground" },
      { text: "INFO  Listening on 0.0.0.0:9000", color: "text-accent" },
      { text: "INFO  Block #2,500,001 finalized (0.42s)", color: "text-primary/80" },
      { text: "INFO  Block #2,500,002 finalized (0.38s)", color: "text-primary/80" },
      { text: "INFO  AI threat scan: no anomalies", color: "text-muted-foreground" },
      { text: "INFO  Network health: OPTIMAL", color: "text-green-400" },
    ],
  },
  {
    label: "Transaction",
    lines: [
      { text: "// NOVAI native transaction format", color: "text-muted-foreground/60" },
      { text: "{", color: "text-foreground" },
      { text: '  "type": "ValueTransfer",', color: "text-accent" },
      { text: '  "from": "novai1qxy2...f8e9",', color: "text-green-400" },
      { text: '  "to": "novai1abc3...d7k2",', color: "text-green-400" },
      { text: '  "amount": "1000000",', color: "text-primary" },
      { text: '  "nonce": 42,', color: "text-foreground/70" },
      { text: '  "ai_signal": true', color: "text-accent" },
      { text: "}", color: "text-foreground" },
    ],
  },
  {
    label: "AI Agent",
    lines: [
      { text: "// Register an AI entity on NOVAI", color: "text-muted-foreground/60" },
      { text: "{", color: "text-foreground" },
      { text: '  "type": "RegisterAIEntity",', color: "text-accent" },
      { text: '  "entity_class": "SecurityMonitor",', color: "text-green-400" },
      { text: '  "capabilities": [', color: "text-foreground/70" },
      { text: '    "threat_detection",', color: "text-primary" },
      { text: '    "anomaly_scoring",', color: "text-primary" },
      { text: '    "adaptive_response"', color: "text-primary" },
      { text: "  ],", color: "text-foreground" },
      { text: '  "governance_bound": true', color: "text-accent" },
      { text: "}", color: "text-foreground" },
    ],
  },
];

export default function CodeShowcase() {
  const [activeTab, setActiveTab] = useState(0);
  const [visibleLines, setVisibleLines] = useState(0);
  const [copied, setCopied] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const isInView = useInView(ref, { once: true, margin: "-100px" });
  const [hasStarted, setHasStarted] = useState(false);

  useEffect(() => {
    if (isInView && !hasStarted) setHasStarted(true);
  }, [isInView, hasStarted]);

  useEffect(() => {
    setVisibleLines(0);
    if (!hasStarted) return;

    const lines = TABS[activeTab].lines;
    let current = 0;
    const interval = setInterval(() => {
      current++;
      setVisibleLines(current);
      if (current >= lines.length) clearInterval(interval);
    }, 80);

    return () => clearInterval(interval);
  }, [activeTab, hasStarted]);

  const handleCopy = () => {
    const text = TABS[activeTab].lines.map((l) => l.text).join("\n");
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div ref={ref}>
      <ScrollSection>
        <div className="text-center mb-12">
          <span className="pill-badge mb-6">For Developers</span>
          <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
            Build on <span className="gradient-text">NOVAI</span>
          </h2>
          <p className="text-muted-foreground max-w-lg mx-auto">
            Purpose-built transaction types for the AI era.
          </p>
        </div>
      </ScrollSection>

      <ScrollSection delay={0.1}>
        <div className="max-w-3xl mx-auto">
          <div
            className="rounded-2xl overflow-hidden"
            style={{
              background: "hsla(228, 40%, 3%, 0.9)",
              border: "1px solid hsla(224, 20%, 18%, 0.6)",
              boxShadow: "0 25px 60px rgba(0, 0, 0, 0.5), 0 0 40px rgba(76, 111, 255, 0.08)",
            }}
          >
            {/* Title bar */}
            <div
              className="flex items-center justify-between px-4 sm:px-5 py-3"
              style={{ borderBottom: "1px solid hsla(224, 20%, 18%, 0.4)" }}
            >
              <div className="flex items-center gap-2">
                <div className="flex gap-1.5">
                  <div className="h-3 w-3 rounded-full" style={{ background: "hsl(0, 70%, 50%)" }} />
                  <div className="h-3 w-3 rounded-full" style={{ background: "hsl(40, 70%, 50%)" }} />
                  <div className="h-3 w-3 rounded-full" style={{ background: "hsl(120, 50%, 45%)" }} />
                </div>
                <span className="text-xs text-muted-foreground ml-3 font-mono hidden sm:inline">
                  novai-protocol
                </span>
              </div>

              <div className="flex items-center gap-2">
                <div className="flex gap-1 overflow-x-auto">
                  {TABS.map((tab, i) => (
                    <button
                      key={tab.label}
                      onClick={() => setActiveTab(i)}
                      className={`px-2.5 sm:px-3 py-1 rounded-md text-[11px] sm:text-xs font-mono transition-all whitespace-nowrap ${
                        activeTab === i
                          ? "text-foreground bg-primary/10"
                          : "text-muted-foreground hover:text-foreground"
                      }`}
                    >
                      {tab.label}
                    </button>
                  ))}
                </div>

                <button
                  onClick={handleCopy}
                  className="ml-1 p-1.5 rounded-md text-muted-foreground hover:text-foreground transition-colors"
                  style={{ background: "hsla(224, 28%, 10%, 0.6)" }}
                >
                  {copied ? <Check size={14} className="text-green-400" /> : <Copy size={14} />}
                </button>
              </div>
            </div>

            {/* Code content */}
            <div className="p-4 sm:p-6 min-h-[300px] sm:min-h-[340px] font-mono text-xs sm:text-sm overflow-hidden">
              <AnimatePresence mode="wait">
                <motion.div
                  key={activeTab}
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: 0.2 }}
                >
                  {TABS[activeTab].lines.map((line, i) => (
                    <div
                      key={i}
                      className={`leading-relaxed transition-opacity duration-150 ${
                        i < visibleLines ? "opacity-100" : "opacity-0"
                      } ${line.color} ${line.text === "" ? "h-4" : ""}`}
                    >
                      {line.text || "\u00A0"}
                    </div>
                  ))}
                  {visibleLines < TABS[activeTab].lines.length && (
                    <span className="inline-block w-2 h-4 bg-accent animate-blink" />
                  )}
                </motion.div>
              </AnimatePresence>
            </div>
          </div>
        </div>
      </ScrollSection>
    </div>
  );
}
