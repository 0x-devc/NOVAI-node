import { motion } from "framer-motion";
import { ExternalLink, ArrowRight } from "lucide-react";
import { Link } from "react-router-dom";
import { XIcon, DiscordIcon, TelegramIcon } from "@/components/novai/SocialIcons";
import ScrollSection from "@/components/novai/ScrollSection";
import Footer from "@/components/novai/Footer";

const EASE = [0.25, 0.46, 0.45, 0.94] as const;

const SOCIALS = [
  { href: "https://x.com/NOVAInetwork", title: "X (Twitter)", description: "Updates, announcements, and progress in public.", icon: XIcon, followers: "Growing", color: "228, 100%, 62%" },
  { href: "https://discord.gg/NTWr6x2dbM", title: "Discord", description: "Main community hub — ideas, feedback, and discussions.", icon: DiscordIcon, followers: "Active", color: "235, 86%, 65%" },
  { href: "https://t.me/+QoacVmowWNRkZjNk", title: "Telegram", description: "Updates and announcements.", icon: TelegramIcon, followers: "Live", color: "200, 90%, 55%" },
];

export default function SocialsPage() {
  return (
    <div className="relative">
      <section className="relative min-h-[60vh] flex items-center overflow-hidden">
        <div className="absolute inset-0 mesh-gradient pointer-events-none" />
        <div className="section-container relative z-10 pt-28 pb-12">
          <motion.div initial={{ opacity: 0, y: 30 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.8, ease: EASE }} className="text-center max-w-2xl mx-auto">
            <span className="pill-badge mb-8">Community</span>
            <h1 className="font-display text-5xl font-bold sm:text-6xl mb-6">
              {"Connect with".split(" ").map((word, i) => (
                <motion.span key={i} initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.2 + i * 0.08, ease: "easeOut" }} className="inline-block mr-[0.3em]">{word}</motion.span>
              ))}
              <motion.span initial={{ opacity: 0, y: 12, filter: "blur(4px)" }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }} transition={{ duration: 0.4, delay: 0.36, ease: "easeOut" }} className="inline-block gradient-text">NOVAInetwork</motion.span>
            </h1>
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

      {/* Stay in the loop */}
      <section className="relative py-24">
        <div className="gradient-divider" />
        <div className="section-container py-20">
          <ScrollSection>
            <div className="text-center max-w-xl mx-auto">
              <h2 className="font-display text-3xl font-bold sm:text-4xl mb-4">The community is <span className="gradient-text">growing</span></h2>
              <p className="text-muted-foreground mb-8">NOVAInetwork is built in public. Every update, every milestone, shared with the community first.</p>
              <div className="flex flex-wrap justify-center gap-4">
                <Link to="/vision" className="btn-ghost no-underline">Read Our Vision <ArrowRight size={16} /></Link>
                <Link to="/documents" className="btn-ghost no-underline">Documentation <ArrowRight size={16} /></Link>
              </div>
            </div>
          </ScrollSection>
        </div>
      </section>

      <Footer />
    </div>
  );
}
