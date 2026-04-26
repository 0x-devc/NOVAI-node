import { motion } from "framer-motion";
import { FileText, ArrowRight, ExternalLink } from "lucide-react";
import { Link } from "react-router-dom";
import ScrollSection from "@/components/novai/ScrollSection";
import Footer from "@/components/novai/Footer";

const DOCUMENTS = [
  {
    href: "https://github.com/0x-devc/NOVAI-node/blob/main/docs/tutorials/FIRST_AI_ENTITY.md",
    title: "Quick Start Tutorial",
    description: "Build your first AI entity in 10 minutes — keygen, faucet, register, publish a signal, query state.",
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
    description: "How a single function rejecting trailing bytes silently killed every block — a debugging story.",
  },
];

export default function DocumentsPage() {
  return (
    <div className="relative">
      {/* Hero */}
      <section className="relative min-h-[55vh] flex items-center overflow-hidden">
        <div className="absolute inset-0 mesh-gradient pointer-events-none" />
        <div className="absolute inset-0 grid-bg-fine opacity-15 pointer-events-none" />

        <div className="section-container relative z-10 pt-28 pb-16 text-center">
          <motion.div initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.8 }}>
            <span className="pill-badge mb-8"><FileText size={12} /> Documentation</span>
            <h1 className="font-display text-5xl font-bold sm:text-6xl mb-6">
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.2, ease: "easeOut" }} className="inline-block mr-[0.3em]">Documents</motion.span>{" "}
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.36, ease: "easeOut" }} className="inline-block gradient-text">&amp; Resources</motion.span>
            </h1>
            <p className="text-lg text-muted-foreground max-w-2xl mx-auto">
              Tutorials, references, SDK examples, and engineering write-ups. All open source on GitHub.
            </p>
          </motion.div>
        </div>
      </section>

      {/* Document grid */}
      <section className="relative pb-24">
        <div className="section-container relative z-10 max-w-5xl mx-auto">
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

      {/* CTA */}
      <section className="relative py-20">
        <div className="gradient-divider" />
        <div className="section-container py-20">
          <ScrollSection>
            <div className="text-center">
              <h3 className="font-display text-2xl font-bold mb-4 text-foreground">Want to dive deeper?</h3>
              <p className="text-muted-foreground mb-8 max-w-md mx-auto">Explore our vision page for detailed insights into the future of NOVAInetwork.</p>
              <div className="flex flex-wrap justify-center gap-4">
                <Link to="/vision" className="btn-ghost no-underline">Read Our Vision <ArrowRight size={16} /></Link>
                <Link to="/socials" className="btn-ghost no-underline">Join Community <ArrowRight size={16} /></Link>
              </div>
            </div>
          </ScrollSection>
        </div>
      </section>

      <Footer />
    </div>
  );
}
