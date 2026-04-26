import { useState, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { Menu, X } from "lucide-react";
import NovaiLogo from "./NovaiLogo";

const NAV_ITEMS = [
  { href: "#home", label: "Home" },
  { href: "#vision", label: "Vision" },
  { href: "#socials", label: "Socials" },
  { href: "#testnet", label: "Testnet" },
  { href: "#documents", label: "Documents" },
];

export default function Navbar() {
  const [mobileOpen, setMobileOpen] = useState(false);
  const [scrolled, setScrolled] = useState(false);
  const [activeSection, setActiveSection] = useState("home");

  useEffect(() => {
    const handleScroll = () => {
      setScrolled(window.scrollY > 20);

      // Detect which section is in view
      const sections = NAV_ITEMS.map(item => item.href.slice(1));
      for (let i = sections.length - 1; i >= 0; i--) {
        const el = document.getElementById(sections[i]);
        if (el) {
          const rect = el.getBoundingClientRect();
          if (rect.top <= 150) {
            setActiveSection(sections[i]);
            break;
          }
        }
      }
    };
    window.addEventListener("scroll", handleScroll);
    return () => window.removeEventListener("scroll", handleScroll);
  }, []);

  const scrollTo = (href: string) => {
    setMobileOpen(false);
    const id = href.slice(1);
    const el = document.getElementById(id);
    if (el) {
      const offset = 64; // navbar height
      const y = el.getBoundingClientRect().top + window.scrollY - offset;
      window.scrollTo({ top: y, behavior: "smooth" });
    }
  };

  return (
    <>
      <header
        className={`fixed top-0 left-0 right-0 z-50 transition-all duration-300 ${scrolled ? "border-b border-border/50" : "border-b border-transparent"}`}
        style={{
          backdropFilter: "blur(16px)",
          WebkitBackdropFilter: "blur(16px)",
          background: scrolled ? "hsla(228, 30%, 4%, 0.85)" : "hsla(228, 30%, 4%, 0.4)",
          boxShadow: scrolled ? "0 1px 30px -10px hsla(228, 100%, 62%, 0.08)" : "none",
        }}
      >
        <nav className="section-container flex items-center justify-between py-4">
          <button onClick={() => scrollTo("#home")} className="flex items-center gap-2.5 no-underline bg-transparent border-none cursor-pointer">
            <NovaiLogo size={32} />
            <span className="font-display text-lg font-bold gradient-text">NOVAInetwork</span>
          </button>

          <div className="hidden items-center gap-1 md:flex">
            {NAV_ITEMS.map((item) => {
              const isActive = activeSection === item.href.slice(1);
              return (
                <button
                  key={item.href}
                  onClick={() => scrollTo(item.href)}
                  className={`relative rounded-lg px-4 py-2 text-sm font-medium transition-colors no-underline bg-transparent border-none cursor-pointer ${isActive ? "text-foreground" : "text-muted-foreground hover:text-foreground"}`}
                >
                  {item.label}
                  {isActive && (
                    <motion.div
                      layoutId="navbar-indicator"
                      className="absolute inset-0 rounded-lg"
                      style={{ background: "hsla(228, 100%, 62%, 0.08)", border: "1px solid hsla(228, 100%, 62%, 0.15)" }}
                      transition={{ type: "spring", bounce: 0.2, duration: 0.5 }}
                    />
                  )}
                </button>
              );
            })}
          </div>

          <button onClick={() => setMobileOpen(!mobileOpen)} className="flex items-center justify-center rounded-lg p-2 text-muted-foreground transition-colors hover:text-foreground md:hidden bg-transparent border-none cursor-pointer" aria-label="Toggle menu">
            {mobileOpen ? <X size={22} /> : <Menu size={22} />}
          </button>
        </nav>
      </header>

      <AnimatePresence>
        {mobileOpen && (
          <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="fixed inset-0 z-40 md:hidden" style={{ background: "hsla(228, 30%, 4%, 0.95)", backdropFilter: "blur(20px)" }}>
            <div className="flex flex-col items-center justify-center h-full gap-6">
              {NAV_ITEMS.map((item, i) => {
                const isActive = activeSection === item.href.slice(1);
                return (
                  <motion.div key={item.href} initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: i * 0.08 }}>
                    <button onClick={() => scrollTo(item.href)} className={`font-display text-2xl font-semibold no-underline transition-colors bg-transparent border-none cursor-pointer ${isActive ? "gradient-text" : "text-muted-foreground hover:text-foreground"}`}>
                      {item.label}
                    </button>
                  </motion.div>
                );
              })}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}
