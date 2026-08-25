import { useState } from "react";
import { LazyMotion, domAnimation, m, motion } from "framer-motion";
import { Code2 } from "lucide-react";
import AnimatedCounter from "@/components/novai/AnimatedCounter";
import ScrollSection from "@/components/novai/ScrollSection";
import SectionHeader from "@/components/system/SectionHeader";
import StatTile from "@/components/system/StatTile";
import MonoLabel from "@/components/system/MonoLabel";
import Reveal from "@/components/system/Reveal";
import LiveChainStats from "@/components/system/LiveChainStats";
import { STATS } from "@/data/stats";
import repoStats from "@/data/repo-stats.generated.json";
import chainSnapshot from "@/data/chain-snapshot.json";
import { floorTo } from "@/lib/format";
import { fadeRise, statSettle, statusTransition } from "@/lib/motion";
import { usePrefersReducedMotion } from "@/hooks/usePrefersReducedMotion";
import { useChainStatus, type ChainStatus } from "@/hooks/useChainStatus";

// Dev-only specimen. Served by the dev server only; absent from the
// production build by construction (no build entry). Chain data arrives
// through the dev proxy at /rpc (vite.config server.proxy), which also does
// not exist in production.

const TESTS_CLAIM = `${floorTo(repoStats.tests.value, 100).toLocaleString()}+`;

const NAV = [
  ["01", "live", "Live chain"],
  ["02", "compare", "Comparison"],
  ["03", "tokens", "Tokens"],
  ["04", "type", "Type"],
  ["05", "accents", "Accents"],
  ["06", "motion", "Motion"],
  ["07", "glow", "Glow"],
] as const;

function Section({ id, num, title, children }: { id: string; num: string; title: string; children: React.ReactNode }) {
  return (
    <section id={id} className="scroll-mt-16 border-t border-line-subtle pt-10 pb-14">
      <div className="flex items-baseline gap-3 mb-8">
        <span className="font-mono text-label text-ink-faint">{num}</span>
        <h2 className="font-display text-h3x font-semibold text-ink-hi">{title}</h2>
      </div>
      {children}
    </section>
  );
}

type ForcedState = "auto" | "loading" | "live" | "stale" | "unreachable";

function forcedStatus(kind: Exclude<ForcedState, "auto">, real: ChainStatus): ChainStatus {
  const snapshot = real.snapshot;
  const base = { snapshot, txCount: 0, failures: 0 };
  switch (kind) {
    case "loading":
      return { ...base, state: "loading", height: null, round: null, bps: null, ageSeconds: null };
    case "live":
      return {
        ...base,
        state: "live",
        height: real.height ?? snapshot.height,
        round: real.round ?? snapshot.round,
        bps: real.bps ?? 1.12,
        ageSeconds: real.ageSeconds ?? 2,
      };
    case "stale":
      return { ...base, state: "stale", height: real.height ?? snapshot.height, round: 0, bps: null, ageSeconds: 754 };
    case "unreachable":
      return { ...base, state: "unreachable", height: null, round: null, bps: null, ageSeconds: null, failures: 5 };
  }
}

function LiveChainSection() {
  const real = useChainStatus({ rpcUrl: "/rpc" });
  const [forced, setForced] = useState<ForcedState>("auto");
  const shown = forced === "auto" ? real : forcedStatus(forced, real);
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <MonoLabel className="mr-2">state</MonoLabel>
        {(["auto", "loading", "live", "stale", "unreachable"] as const).map((k) => (
          <button
            key={k}
            onClick={() => setForced(k)}
            className={`rounded-md border px-3 py-1 text-xs font-mono transition-colors ${
              forced === k ? "border-brand text-ink-hi bg-surface-2" : "border-line text-ink-low hover:text-ink-mid"
            }`}
          >
            {k}
          </button>
        ))}
        {forced !== "auto" && (
          <span className="text-xs font-mono uppercase tracking-[0.05em] text-warnx-text ml-2">forced preview</span>
        )}
      </div>
      <LiveChainStats status={shown} />
      <p className="text-xs text-ink-low">
        Auto state is the real chain through the dev proxy: browser to localhost /rpc to rpc.novai.network. The
        proxy lives in the vite dev server config only and is absent from the production bundle. Snapshot fallback
        captured {chainSnapshot.capturedAt.slice(0, 10)} at height {chainSnapshot.height.toLocaleString()}.
      </p>
    </div>
  );
}

function ProposedStats() {
  return (
    <div className="border border-line-subtle rounded-lg overflow-hidden">
      <div className="bg-surface-0 px-8" style={{ padding: "var(--sp-2xl) 2rem" }}>
        <Reveal>
          <SectionHeader
            kicker="Private testnet"
            title="Numbers that matter"
            lede="Clean-room engineering. Every line built from scratch."
          />
        </Reveal>
        <Reveal className="mt-10">
          <div className="grid sm:grid-cols-3 divide-y sm:divide-y-0 sm:divide-x divide-line-subtle border-y border-line-subtle">
            <StatTile value="Active" label="Private testnet running" provenance="static status" />
            <StatTile value="4" label="Validators" provenance="fleet configuration" />
            <StatTile value={TESTS_CLAIM} label="Tests" provenance="counted from source at build" />
          </div>
        </Reveal>
        <Reveal className="mt-8">
          <p className="text-bodyx text-ink-mid max-w-xl">
            Built from scratch in Rust. <span className="text-ink-hi">No forks. No copied code.</span>
          </p>
        </Reveal>
      </div>
    </div>
  );
}

function CurrentStats() {
  return (
    <div className="border border-line-subtle rounded-lg overflow-hidden">
      <div className="relative py-16" style={{ background: "hsl(228 30% 4%)" }}>
        <div className="section-container relative z-10">
          <ScrollSection>
            <div className="text-center mb-12">
              <span className="pill-badge mb-6">Private Testnet Metrics</span>
              <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
                Numbers that <span className="gradient-text">matter</span>
              </h2>
              <p className="text-muted-foreground">Clean-room engineering. Every line built from scratch.</p>
            </div>
          </ScrollSection>
          <div className="grid gap-8 sm:grid-cols-3">
            {STATS.map((stat) => (
              <ScrollSection key={stat.label}>
                <div className="text-center">
                  <div className="font-display text-5xl font-bold gradient-text mb-3">
                    {stat.textOnly ? (
                      <motion.span
                        initial={{ opacity: 0, y: 10 }}
                        whileInView={{ opacity: 1, y: 0 }}
                        viewport={{ once: true }}
                        transition={{ duration: 0.5 }}
                        className="font-display"
                      >
                        {stat.textOnly}
                      </motion.span>
                    ) : (
                      <AnimatedCounter target={stat.value!} suffix={stat.suffix} prefix={stat.prefix} />
                    )}
                  </div>
                  <p className="text-sm text-muted-foreground">{stat.label}</p>
                </div>
              </ScrollSection>
            ))}
          </div>
          <ScrollSection>
            <div className="mt-12 flex justify-center">
              <div className="glass-card rounded-2xl px-8 py-5 flex items-center gap-4">
                <Code2 size={24} className="text-accent" />
                <div>
                  <p className="font-display text-lg font-semibold text-foreground">Built from Scratch in Rust</p>
                  <p className="text-xs text-muted-foreground">No forks. No copied code. Pure clean-room engineering.</p>
                </div>
              </div>
            </div>
          </ScrollSection>
        </div>
      </div>
    </div>
  );
}

function MotionDemo({ name, spec, children }: { name: string; spec: string; children: (key: number) => React.ReactNode }) {
  const [key, setKey] = useState(0);
  return (
    <div className="border border-line rounded-lg p-5 bg-surface-1">
      <div className="flex items-center justify-between mb-4">
        <MonoLabel>{name}</MonoLabel>
        <button
          onClick={() => setKey((k) => k + 1)}
          className="text-xs text-brand-text hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-line-focus rounded"
        >
          Replay
        </button>
      </div>
      <div className="h-14 flex items-center">{children(key)}</div>
      <p className="text-xs text-ink-low mt-3">{spec}</p>
    </div>
  );
}

export default function SpecimenApp() {
  const reduced = usePrefersReducedMotion();
  const [statusOn, setStatusOn] = useState(true);
  const ramp = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7", "n8", "n9"];
  return (
    <LazyMotion features={domAnimation}>
      <div className="min-h-screen bg-surface-0 text-ink-hi antialiased">
        <header className="border-b border-line-strong">
          <div className="mx-auto max-w-6xl px-6 py-4">
            <p className="font-display text-h3x font-semibold text-ink-hi">Design specimen</p>
            <p className="text-sm text-ink-mid mt-1">
              Dev-only judging surface: live chain states, the stats section in system v2 beside the current
              production treatment, and the token system on real content. Not linked, not built for production.
            </p>
          </div>
        </header>
        <nav className="sticky top-0 z-nav bg-surface-0/90 backdrop-blur-sm border-b border-line-strong">
          <div className="mx-auto max-w-6xl px-6 py-2.5 flex flex-wrap gap-x-5 gap-y-1">
            {NAV.map(([num, id, label]) => (
              <a key={id} href={`#${id}`} className="text-xs font-mono text-ink-low hover:text-ink-hi no-underline">
                <span className="text-ink-faint mr-1.5">{num}</span>
                {label}
              </a>
            ))}
          </div>
        </nav>

        <main className="mx-auto max-w-6xl px-6 pb-24">
          <Section id="live" num="01" title="Live chain, four states">
            <LiveChainSection />
          </Section>

          <Section id="compare" num="02" title="Stats section: proposed beside current">
            <div className="space-y-6">
              <div>
                <MonoLabel>Proposed, system v2</MonoLabel>
                <div className="mt-3">
                  <ProposedStats />
                </div>
              </div>
              <div>
                <MonoLabel>Current production treatment</MonoLabel>
                <div className="mt-3">
                  <CurrentStats />
                </div>
              </div>
            </div>
          </Section>

          <Section id="tokens" num="03" title="Neutral ramp and text emphasis">
            <div className="grid grid-cols-5 sm:grid-cols-10 gap-2">
              {ramp.map((n) => (
                <div key={n} className="space-y-1.5">
                  <div className="h-12 rounded-md border border-line-subtle" style={{ background: `hsl(var(--${n}))` }} />
                  <p className="text-xs text-ink-low font-mono">{n}</p>
                </div>
              ))}
            </div>
            <div className="grid sm:grid-cols-2 gap-4 mt-6">
              {(["surface-0", "surface-2"] as const).map((ground) => (
                <div key={ground} className={`bg-${ground} border border-line rounded-lg p-5 space-y-1.5`}>
                  <p className="text-ink-hi text-sm">text-hi: headlines and values</p>
                  <p className="text-ink-mid text-sm">text-mid: body copy</p>
                  <p className="text-ink-low text-sm">text-low: labels and provenance</p>
                  <p className="text-ink-faint text-sm">
                    text-faint: decorative only <span className="text-errorx-text">(barred from content)</span>
                  </p>
                  <p className="text-xs text-ink-low font-mono pt-2">on {ground}</p>
                </div>
              ))}
            </div>
          </Section>

          <Section id="type" num="04" title="Type on real words">
            <div className="space-y-5">
              <p className="font-display text-display font-semibold text-ink-hi">The chain proves itself.</p>
              <p className="font-display text-h2x font-semibold text-ink-hi">Numbers that matter</p>
              <p className="font-display text-h3x font-medium text-ink-hi">HotStuff-inspired BFT consensus</p>
              <p className="font-mono text-stat font-light text-ink-hi tabular-nums">{chainSnapshot.height.toLocaleString()}</p>
              <p className="text-bodyx text-ink-mid max-w-xl">
                Body text carries most of the reading. It sits one emphasis level below headlines and stays fully
                readable on every ground it is allowed to sit on.
              </p>
              <MonoLabel>Uppercase mono label, plus five percent tracking</MonoLabel>
            </div>
          </Section>

          <Section id="accents" num="05" title="Accents and their scope">
            <div className="grid sm:grid-cols-3 gap-4">
              <div className="border border-line rounded-lg p-5 bg-surface-1 space-y-3">
                <button
                  className="rounded-md px-4 py-2 text-sm font-semibold text-white"
                  style={{ background: "hsl(var(--brand))", boxShadow: "var(--glow-2)" }}
                >
                  Primary action
                </button>
                <p className="text-sm">
                  <a href="#live" className="text-brand-text hover:underline">
                    brand-text link on dark ground
                  </a>
                </p>
                <p className="text-xs text-ink-low">brand: interactive fills and links only</p>
              </div>
              <div className="border border-line rounded-lg p-5 bg-surface-1 space-y-3">
                <button onClick={() => setStatusOn((v) => !v)} className="flex items-center gap-2 text-left">
                  <m.span
                    animate={{ backgroundColor: statusOn ? "hsl(192 95% 68%)" : "hsl(38 92% 58%)" }}
                    transition={statusTransition(reduced)}
                    className="inline-block h-2.5 w-2.5 rounded-full"
                  />
                  <span className="font-mono text-label uppercase tracking-[0.05em] text-ink-mid">
                    {statusOn ? "live" : "paused"}
                  </span>
                </button>
                <p className="text-xs text-ink-low">
                  live (cyan): live-state only, one element per viewport. Click to see status-transition; instant
                  swap under reduced motion.
                </p>
              </div>
              <div className="border border-line rounded-lg p-5 bg-surface-1 space-y-3">
                <p className="font-display text-h3x font-semibold">
                  A new beginning for <span className="gradient-text">intelligence</span>
                </p>
                <p className="text-xs text-ink-low">
                  violet: third gradient stop only, one gradient per section, zero in quiet sections
                </p>
              </div>
            </div>
          </Section>

          <Section id="motion" num="06" title={`Motion primitives (reduced motion: ${reduced ? "on, reduced states shown" : "off"})`}>
            <div className="grid sm:grid-cols-3 gap-4">
              <MotionDemo name="fade-rise" spec="0.5s, ease-out-quart, y 16px. Reduced: static at final position, opaque, no transform.">
                {(key) => (
                  <m.div key={key} initial="hidden" animate="visible" variants={reduced ? fadeRise.reduced : fadeRise.full} className="text-bodyx text-ink-mid">
                    Section content reveal
                  </m.div>
                )}
              </MotionDemo>
              <MotionDemo name="stat-settle" spec="0.3s opacity + blur to zero. Value never counts from zero. Reduced: final value, no blur, no ramp.">
                {(key) => (
                  <m.div key={key} initial="hidden" animate="visible" variants={reduced ? statSettle.reduced : statSettle.full} className="font-display text-stat font-light text-ink-hi tabular-nums">
                    {TESTS_CLAIM}
                  </m.div>
                )}
              </MotionDemo>
              <MotionDemo name="hover-lift" spec="150ms, y -2px + border-color, CSS only. Reduced: color change only, no movement.">
                {() => (
                  <div className="border border-line rounded-md px-4 py-2 text-sm text-ink-mid transition-all duration-150 hover:-translate-y-0.5 hover:border-line-strong cursor-default">
                    Hover me
                  </div>
                )}
              </MotionDemo>
            </div>
          </Section>

          <Section id="glow" num="07" title="Glow, three capped levels">
            <div className="grid sm:grid-cols-3 gap-4">
              {(["--glow-1", "--glow-2", "--glow-3"] as const).map((g) => (
                <div key={g} className="h-16 rounded-lg border border-line bg-surface-1 flex items-center justify-center" style={{ boxShadow: `var(${g})` }}>
                  <p className="text-xs text-ink-low font-mono">{g}</p>
                </div>
              ))}
            </div>
            <p className="text-xs text-ink-low mt-3">
              glow-3 is hero and commit-flash only. Cards never exceed glow-2. The old triple-100px stack is deleted.
            </p>
          </Section>
        </main>
      </div>
    </LazyMotion>
  );
}
