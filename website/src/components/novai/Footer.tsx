import { Link } from "react-router-dom";
import { DiscordIcon, XIcon, TelegramIcon } from "./SocialIcons";
import NovaiLogo from "./NovaiLogo";

export default function Footer() {
  return (
    <footer className="relative border-t border-border/50">
      <div
        className="absolute inset-0 pointer-events-none"
        style={{
          background: "linear-gradient(180deg, transparent, hsla(228, 30%, 4%, 0.5))",
        }}
      />
      <div className="section-container relative py-16">
        <div className="grid gap-12 md:grid-cols-4">
          {/* Brand */}
          <div className="md:col-span-2">
            <div className="flex items-center gap-2.5 mb-4">
              <NovaiLogo size={28} />
              <span className="font-display text-lg font-bold gradient-text">NOVAInetwork</span>
            </div>
            <p className="max-w-sm text-sm leading-relaxed text-muted-foreground">
              NOVAI — The AI-Integrated Blockchain. The network that Learns, Protects and Evolves.
            </p>
            <p className="max-w-sm text-sm leading-relaxed text-muted-foreground mt-2 italic">
              The intelligent network is awakening.
            </p>
          </div>

          {/* Links */}
          <div>
            <h4 className="mb-4 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              Explore
            </h4>
            <div className="flex flex-col gap-2.5">
              {[
                { to: "/", label: "Home" },
                { to: "/vision", label: "Vision" },
                { to: "/documents", label: "Documents" },
                { to: "/testnet", label: "Testnet" },
              ].map((link) => (
                <Link
                  key={link.to}
                  to={link.to}
                  className="text-sm text-muted-foreground transition-colors hover:text-foreground no-underline"
                >
                  {link.label}
                </Link>
              ))}
            </div>
          </div>

          {/* Social */}
          <div>
            <h4 className="mb-4 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              Community
            </h4>
            <div className="flex gap-3">
              {[
                { href: "https://x.com/NOVAInetwork", icon: <XIcon size={18} />, label: "X" },
                { href: "https://discord.gg/NTWr6x2dbM", icon: <DiscordIcon size={18} />, label: "Discord" },
                { href: "https://t.me/+QoacVmowWNRkZjNk", icon: <TelegramIcon size={18} />, label: "Telegram" },
              ].map((s) => (
                <a
                  key={s.label}
                  href={s.href}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="flex h-10 w-10 items-center justify-center rounded-lg border border-border/50 text-muted-foreground transition-all hover:border-primary/30 hover:text-foreground no-underline"
                  aria-label={s.label}
                >
                  {s.icon}
                </a>
              ))}
            </div>
          </div>
        </div>

        <div className="gradient-divider mt-12 mb-6" />

        <div className="flex flex-col items-center justify-between gap-4 sm:flex-row">
          <p className="text-xs text-muted-foreground">
            © 2025 NOVAInetwork. All rights reserved.
          </p>
          <p className="text-xs text-muted-foreground">
            Built from scratch. No forks. No compromises.
          </p>
        </div>
      </div>
    </footer>
  );
}
