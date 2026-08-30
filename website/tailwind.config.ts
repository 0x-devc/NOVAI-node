import type { Config } from "tailwindcss";

export default {
  darkMode: ["class"],
  // console.html is scanned because the console's sections are static HTML
  // rather than React. Without it every utility class on that page is purged.
  content: [
    "./pages/**/*.{ts,tsx}",
    "./components/**/*.{ts,tsx}",
    "./app/**/*.{ts,tsx}",
    "./src/**/*.{ts,tsx}",
    "./console.html",
    // Every console page, not just the landing one. A page missing from this
    // glob has every utility class on it purged, and the failure is
    // production-only: dev does not purge, so it looks correct until it is
    // deployed. A glob is not a guarantee either, which is why the build
    // asserts that every entry in rollupOptions.input is matched by one of
    // these and that a probe class from each page survives into the built CSS.
    "./console/*.html",
  ],
  prefix: "",
  theme: {
    container: {
      center: true,
      padding: "2rem",
      screens: { "2xl": "1400px" },
    },
    extend: {
      fontFamily: {
        display: ["Space Grotesk Variable", "Space Grotesk", "sans-serif"],
        body: ["Inter Variable", "Inter", "sans-serif"],
        mono: ["ui-monospace", "SFMono-Regular", "Menlo", "Consolas", "monospace"],
      },
      colors: {
        ink: {
          hi: "hsl(var(--text-hi))",
          mid: "hsl(var(--text-mid))",
          low: "hsl(var(--text-low))",
          faint: "hsl(var(--text-faint))",
        },
        line: {
          subtle: "hsl(var(--line-subtle) / 0.5)",
          DEFAULT: "hsl(var(--line-default) / 0.6)",
          strong: "hsl(var(--line-strong) / 0.95)",
          focus: "hsl(var(--line-focus) / 0.7)",
        },
        surface: {
          0: "hsl(var(--n0))",
          1: "hsl(var(--n1))",
          2: "hsl(var(--n2))",
          3: "hsl(var(--n3))",
        },
        brand: {
          DEFAULT: "hsl(var(--brand))",
          text: "hsl(var(--brand-text))",
        },
        live: "hsl(var(--live))",
        warnx: {
          DEFAULT: "hsl(var(--warn))",
          text: "hsl(var(--warn-text))",
        },
        errorx: {
          DEFAULT: "hsl(var(--error))",
          text: "hsl(var(--error-text))",
        },
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
      },
      fontSize: {
        display: ["var(--fs-display)", { lineHeight: "1.05", letterSpacing: "-0.03em" }],
        h2x: ["var(--fs-h2)", { lineHeight: "1.1", letterSpacing: "-0.03em" }],
        h3x: ["var(--fs-h3)", { lineHeight: "1.25", letterSpacing: "-0.02em" }],
        stat: ["var(--fs-stat)", { lineHeight: "1.1", letterSpacing: "-0.02em" }],
        bodyx: ["var(--fs-body)", { lineHeight: "1.6" }],
        label: ["var(--fs-label)", { lineHeight: "1.4", letterSpacing: "0.05em" }],
      },
      boxShadow: {
        glow1: "var(--glow-1)",
        glow2: "var(--glow-2)",
        glow3: "var(--glow-3)",
      },
      zIndex: {
        backdrop: "0",
        content: "10",
        progress: "40",
        nav: "50",
        menu: "60",
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      keyframes: {
        "accordion-down": {
          from: { height: "0" },
          to: { height: "var(--radix-accordion-content-height)" },
        },
        "accordion-up": {
          from: { height: "var(--radix-accordion-content-height)" },
          to: { height: "0" },
        },
      },
      animation: {
        "accordion-down": "accordion-down 0.2s ease-out",
        "accordion-up": "accordion-up 0.2s ease-out",
      },
    },
  },
  plugins: [require("tailwindcss-animate")],
} satisfies Config;
