import { useRef, useState, useEffect } from "react";
import { motion, useInView } from "framer-motion";

const TERMINAL_LINES = [
  { text: "$ novai-node --chain testnet --mode validator", type: "cmd" },
  { text: "INFO  Loading genesis configuration...", type: "info" },
  { text: "INFO  Initializing BFT consensus engine", type: "info" },
  { text: "INFO  AI security module: ACTIVE", type: "success" },
  { text: "INFO  Sparse Merkle Tree state initialized", type: "info" },
  { text: "INFO  Connected to 4 peer validators", type: "info" },
  { text: "INFO  Block #2,500,001 finalized (0.42s)", type: "block" },
  { text: "INFO  Block #2,500,002 finalized (0.38s)", type: "block" },
  { text: "INFO  AI threat scan: no anomalies detected", type: "info" },
  { text: "INFO  Block #2,500,003 finalized (0.41s)", type: "block" },
  { text: "INFO  Network health: OPTIMAL", type: "success" },
];

function getLineColor(type: string) {
  switch (type) {
    case "cmd":
      return "text-accent";
    case "success":
      return "text-green-400";
    case "block":
      return "text-primary/80";
    default:
      return "text-muted-foreground";
  }
}

export default function TiltTerminal() {
  const containerRef = useRef<HTMLDivElement>(null);
  const isInView = useInView(containerRef, { once: true, margin: "-100px" });
  const [lines, setLines] = useState<typeof TERMINAL_LINES>([]);
  const [currentLine, setCurrentLine] = useState(0);
  const [tilt, setTilt] = useState({ x: 0, y: 0 });

  // Type out lines when in view
  useEffect(() => {
    if (!isInView) return;

    if (currentLine >= TERMINAL_LINES.length) {
      const timeout = setTimeout(() => {
        setLines([]);
        setCurrentLine(0);
      }, 3000);
      return () => clearTimeout(timeout);
    }

    const timeout = setTimeout(() => {
      setLines((prev) => [...prev, TERMINAL_LINES[currentLine]]);
      setCurrentLine((prev) => prev + 1);
    }, 500 + Math.random() * 400);

    return () => clearTimeout(timeout);
  }, [isInView, currentLine]);

  const handleMouseMove = (e: React.MouseEvent<HTMLDivElement>) => {
    const el = containerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width - 0.5;
    const y = (e.clientY - rect.top) / rect.height - 0.5;
    setTilt({ x: y * -8, y: x * 8 });
  };

  const handleMouseLeave = () => {
    setTilt({ x: 0, y: 0 });
  };

  return (
    <section className="relative py-20">
      <div className="section-container relative z-10">
        <motion.div
          ref={containerRef}
          initial={{ opacity: 0, y: 60 }}
          animate={isInView ? { opacity: 1, y: 0 } : {}}
          transition={{ duration: 0.8, ease: [0.25, 0.46, 0.45, 0.94] }}
          onMouseMove={handleMouseMove}
          onMouseLeave={handleMouseLeave}
          className="max-w-3xl mx-auto"
          style={{
            perspective: 1000,
          }}
        >
          <motion.div
            animate={{
              rotateX: tilt.x,
              rotateY: tilt.y,
            }}
            transition={{ type: "spring", stiffness: 200, damping: 20 }}
            style={{ transformStyle: "preserve-3d" }}
          >
            <div
              className="rounded-2xl overflow-hidden text-left font-mono text-xs sm:text-sm"
              style={{
                background: "hsla(228, 40%, 3%, 0.9)",
                border: "1px solid hsla(224, 20%, 18%, 0.6)",
                boxShadow:
                  "0 25px 60px -12px rgba(0, 0, 0, 0.5), 0 0 40px -8px hsla(228, 100%, 62%, 0.08)",
              }}
            >
              {/* Title bar */}
              <div
                className="flex items-center gap-2 px-4 py-3"
                style={{
                  borderBottom: "1px solid hsla(224, 20%, 18%, 0.4)",
                }}
              >
                <div className="flex gap-1.5">
                  <div
                    className="h-3 w-3 rounded-full"
                    style={{ background: "hsl(0, 70%, 50%)" }}
                  />
                  <div
                    className="h-3 w-3 rounded-full"
                    style={{ background: "hsl(40, 70%, 50%)" }}
                  />
                  <div
                    className="h-3 w-3 rounded-full"
                    style={{ background: "hsl(120, 50%, 45%)" }}
                  />
                </div>
                <span className="text-[11px] text-muted-foreground ml-2 font-medium">
                  novai-testnet - validator node
                </span>
              </div>

              {/* Terminal content */}
              <div className="p-5 h-[300px] overflow-hidden">
                {lines.map((line, i) => (
                  <motion.div
                    key={`${i}-${line.text}`}
                    initial={{ opacity: 0, x: -8 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ duration: 0.25 }}
                    className={`mb-1.5 leading-relaxed ${getLineColor(line.type)}`}
                  >
                    {line.text}
                  </motion.div>
                ))}
                {isInView && currentLine < TERMINAL_LINES.length && (
                  <span className="inline-block w-2 h-4 bg-accent animate-blink" />
                )}
              </div>
            </div>
          </motion.div>
        </motion.div>
      </div>
    </section>
  );
}
