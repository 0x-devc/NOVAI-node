import { motion } from "framer-motion";
import { FileText, ArrowRight, Clock } from "lucide-react";
import { Link } from "react-router-dom";
import ScrollSection from "@/components/novai/ScrollSection";
import Footer from "@/components/novai/Footer";

export default function DocumentsPage() {
  return (
    <div className="relative">
      {/* Hero */}
      <section className="relative min-h-[65vh] flex items-center overflow-hidden">
        <div className="absolute inset-0 mesh-gradient pointer-events-none" />
        <div className="absolute inset-0 grid-bg-fine opacity-15 pointer-events-none" />

        <div className="section-container relative z-10 pt-28 pb-16 text-center">
          <motion.div initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.8 }}>
            <span className="pill-badge mb-8"><FileText size={12} /> Documentation</span>
            <h1 className="font-display text-5xl font-bold sm:text-6xl mb-6">
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.2, ease: "easeOut" }} className="inline-block mr-[0.3em]">Documents</motion.span>{" "}
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.36, ease: "easeOut" }} className="inline-block gradient-text">Coming Soon</motion.span>
            </h1>
            <p className="text-lg text-muted-foreground max-w-2xl mx-auto">
              We're preparing comprehensive documentation including our litepaper,
              technical architecture specs, and tokenomics. Stay tuned for updates.
            </p>
          </motion.div>
        </div>
      </section>

      {/* Coming Soon Card */}
      <section className="relative pb-20">
        <div className="section-container relative z-10 max-w-xl mx-auto">
          <ScrollSection>
            <div className="glass-card-hover glow-border rounded-2xl p-10 sm:p-14 relative overflow-hidden text-center">
              <div className="absolute top-0 left-0 right-0 h-[2px]" style={{ background: "linear-gradient(90deg, hsl(228, 100%, 62%), hsl(192, 95%, 68%))" }} />
              <div className="flex flex-col items-center gap-6">
                <div className="flex h-20 w-20 items-center justify-center rounded-2xl" style={{ background: "hsla(228, 100%, 62%, 0.08)", border: "1px solid hsla(228, 100%, 62%, 0.15)" }}>
                  <FileText size={36} className="text-primary/50" />
                </div>
                <div>
                  <h2 className="font-display text-2xl font-bold mb-3 text-foreground">Documents Coming Soon</h2>
                  <p className="text-sm text-muted-foreground leading-relaxed max-w-md mx-auto">
                    Our litepaper, technical architecture specs, tokenomics model, and developer SDK documentation are being finalized.
                  </p>
                </div>
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <Clock size={12} />
                  <span>In progress</span>
                </div>
              </div>
            </div>
          </ScrollSection>
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
