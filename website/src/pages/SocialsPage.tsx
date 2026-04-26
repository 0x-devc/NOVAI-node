import { motion } from "framer-motion";
import { ExternalLink } from "lucide-react";
import { XIcon, DiscordIcon, TelegramIcon } from "@/components/novai/SocialIcons";
import ScrollSection from "@/components/novai/ScrollSection";
import Footer from "@/components/novai/Footer";

const SOCIALS = [
  {
    href: "https://x.com/NOVAInetwork",
    title: "X (Twitter)",
    description: "Updates, announcements, and progress in public.",
    icon: XIcon,
    followers: "Growing",
    color: "228, 100%, 62%",
  },
  {
    href: "https://discord.gg/NTWr6x2dbM",
    title: "Discord",
    description: "Main community hub — ideas, feedback, and discussions.",
    icon: DiscordIcon,
    followers: "Active",
    color: "235, 86%, 65%",
  },
  {
    href: "https://t.me/+QoacVmowWNRkZjNk",
    title: "Telegram",
    description: "Updates and announcements.",
    icon: TelegramIcon,
    followers: "Live",
    color: "200, 90%, 55%",
  },
];

export default function SocialsPage() {
  return (
    <div className="relative">
      <section className="relative min-h-[60vh] flex items-center overflow-hidden">
        <div className="absolute inset-0 mesh-gradient pointer-events-none" />
        <div className="section-container relative z-10 pt-28 pb-12">
          <motion.div
            initial={{ opacity: 0, y: 30 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.8 }}
            className="text-center max-w-2xl mx-auto"
          >
            <span className="pill-badge mb-8">Community</span>
            <h1 className="font-display text-5xl font-bold sm:text-6xl mb-6">
              Connect with <span className="gradient-text">NOVAInetwork</span>
            </h1>
            <p className="text-lg text-muted-foreground">
              Join the community building the future of intelligent blockchain infrastructure.
            </p>
          </motion.div>
        </div>
      </section>

      <section className="relative pb-32">
        <div className="section-container relative z-10 max-w-2xl mx-auto">
          <div className="flex flex-col gap-5">
            {SOCIALS.map((social, i) => (
              <ScrollSection key={social.title} delay={i * 0.1}>
                <a
                  href={social.href}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="glass-card-hover glow-border rounded-2xl p-6 flex items-center justify-between group no-underline transition-all"
                >
                  <div className="flex items-center gap-5">
                    <div
                      className="flex h-14 w-14 items-center justify-center rounded-xl transition-all duration-300"
                      style={{
                        background: `hsla(${social.color}, 0.1)`,
                        border: `1px solid hsla(${social.color}, 0.2)`,
                      }}
                    >
                      <social.icon size={24} />
                    </div>
                    <div>
                      <h3 className="font-display text-base font-semibold text-foreground mb-0.5">
                        {social.title}
                      </h3>
                      <p className="text-sm text-muted-foreground">{social.description}</p>
                    </div>
                  </div>

                  <div className="flex items-center gap-3">
                    <span className="hidden sm:inline-block pill-badge text-[10px]">{social.followers}</span>
                    <ExternalLink
                      size={16}
                      className="text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5 group-hover:text-foreground"
                    />
                  </div>
                </a>
              </ScrollSection>
            ))}
          </div>
        </div>
      </section>

      <Footer />
    </div>
  );
}
