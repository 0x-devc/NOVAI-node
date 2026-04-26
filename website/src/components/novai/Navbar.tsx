import { useState, useEffect } from "react";
import { Link, useLocation } from "react-router-dom";
import { motion, AnimatePresence } from "framer-motion";
import { Menu, X } from "lucide-react";
import NovaiLogo from "./NovaiLogo";

const NAV_ITEMS = [
  { href: "/", label: "Home" },
  { href: "/vision", label: "Vision" },
  { href: "/socials", label: "Socials" },
  { href: "/testnet", label: "Testnet" },
  { href: "/documents", label: "Documents" },
];

export default function Navbar() {
  const location = useLocation();
  const [mobileOpen, setMobileOpen] = useState(false);
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const handleScroll = () => setScrolled(window.scrollY > 20);
    window.addEventListener("scroll", handleScroll);
    return () => window.removeEventListener("scroll", handleScroll);
  }, []);

  useEffect(() => {
    setMobileOpen(false);
  }, [location.pathname]);

  return (
    <>
      <header
        className={`fixed top-0 left-0 right-0 z-50 transition-all duration-300 ${
          scrolled
            ? "border-b border-border/50"
            : "border-b border-transparent"
        }`}
        style={{
          backdropFilter: "blur(16px)",
          WebkitBackdropFilter: "blur(16px)",
          background: scrolled
            ? "hsla(228, 30%, 4%, 0.85)"
            : "hsla(228, 30%, 4%, 0.4)",
          boxShadow: scrolled
            ? "0 1px 30px -10px hsla(228, 100%, 62%, 0.08)"
            : "none",
        }}
      >
        <nav className="section-container flex items-center justify-between py-4">
          <Link to="/" className="flex items-center gap-2.5 no-underline">
            <NovaiLogo size={32} />
            <span className="font-display text-lg font-bold gradient-text">
              NOVAInetwork
            </span>
          </Link>

          {/* Desktop nav */}
          <div className="hidden items-center gap-1 md:flex">
            {NAV_ITEMS.map((item) => {
              const isActive = location.pathname === item.href;
              return (
                <Link
                  key={item.href}
                  to={item.href}
                  className={`relative rounded-lg px-4 py-2 text-sm font-medium transition-colors no-underline ${
                    isActive
                      ? "text-foreground"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                >
                  {item.label}
                  {isActive && (
                    <motion.div
                      layoutId="navbar-indicator"
                      className="absolute inset-0 rounded-lg"
                      style={{
                        background: "hsla(228, 100%, 62%, 0.08)",
                        border: "1px solid hsla(228, 100%, 62%, 0.15)",
                      }}
                      transition={{ type: "spring", bounce: 0.2, duration: 0.5 }}
                    />
                  )}
                </Link>
              );
            })}
          </div>

          {/* Mobile menu button */}
          <button
            onClick={() => setMobileOpen(!mobileOpen)}
            className="flex items-center justify-center rounded-lg p-2 text-muted-foreground transition-colors hover:text-foreground md:hidden"
            aria-label="Toggle menu"
          >
            {mobileOpen ? <X size={22} /> : <Menu size={22} />}
          </button>
        </nav>
      </header>

      {/* Mobile overlay */}
      <AnimatePresence>
        {mobileOpen && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-40 md:hidden"
            style={{ background: "hsla(228, 30%, 4%, 0.95)", backdropFilter: "blur(20px)" }}
          >
            <div className="flex flex-col items-center justify-center h-full gap-6">
              {NAV_ITEMS.map((item, i) => {
                const isActive = location.pathname === item.href;
                return (
                  <motion.div
                    key={item.href}
                    initial={{ opacity: 0, y: 20 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ delay: i * 0.08 }}
                  >
                    <Link
                      to={item.href}
                      className={`font-display text-2xl font-semibold no-underline transition-colors ${
                        isActive ? "gradient-text" : "text-muted-foreground hover:text-foreground"
                      }`}
                    >
                      {item.label}
                    </Link>
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
