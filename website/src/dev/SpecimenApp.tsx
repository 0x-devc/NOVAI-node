import { useState } from "react";
import { LazyMotion, domAnimation, m, motion } from "framer-motion";
import { Code2 } from "lucide-react";
import AnimatedCounter from "@/components/novai/AnimatedCounter";
import ScrollSection from "@/components/novai/ScrollSection";
import SectionHeader from "@/components/system/SectionHeader";
import StatTile from "@/components/system/StatTile";
import MonoLabel from "@/components/system/MonoLabel";
import Reveal from "@/components/system/Reveal";
import { STATS } from "@/data/stats";
import repoStats from "@/data/repo-stats.generated.json";
import { floorTo } from "@/lib/format";
import { fadeRise, statSettle, statusTransition } from "@/lib/motion";
import { usePrefersReducedMotion } from "@/hooks/usePrefersReducedMotion";

// Dev-only specimen: the stats section rebuilt in system v2, above the current
// production treatment of the same section, on the same derived data, plus a
// compact system reference. This file is served by the dev server only; the
// production build has no specimen entry.

const TESTS_CLAIM = `${floorTo(repoStats.tests.value, 100).toLocaleString()}+`;

function PaneLabel({ children }: { children: string }) {
  return (
    <div className="sticky top-0 z-nav bg-surface-0/90 backdrop-blur-sm border-b border-line-strong px-6 py-2">
      <MonoLabel className="text-ink-mid">{children}</MonoLabel>
    </div>
  );
}

function ProposedStats() {
  return (
    <section className="bg-surface-0" style={{ padding: "var(--section-pad) 0" }}>
      <div className="mx-auto max-w-7xl px-6 sm:px-8">
        <Reveal>
          <SectionHeader
            kicker="Private testnet"
            title="Numbers that matter"
            lede="Clean-room engineering. Every line built from scratch."
          />
        </Reveal>
        <Reveal className="mt-12">
          <div className="grid sm:grid-cols-3 divide-y sm:divide-y-0 sm:divide-x divide-line-subtle border-y border-line-subtle">
            <StatTile value="Active" label="Private testnet running" provenance="static status" />
            <StatTile value="4" label="Validators" provenance="fleet configuration" />
            <StatTile value={TESTS_CLAIM} label="Tests" provenance="counted from source at build" />
          </div>
        </Reveal>
        <Reveal className="mt-10">
          <p className="text-bodyx text-ink-mid max-w-xl">
            Built from scratch in Rust. <span className="text-ink-hi">No forks. No copied code.</span>
          </p>
        </Reveal>
      </div>
    </section>
  );
}

function CurrentStats() {
  return (
    <section className="relative py-24" style={{ background: "hsl(228 30% 4%)" }}>
      <div className="gradient-divider" />
      <div className="section-container relative z-10 py-20">
        <ScrollSection>
          <div className="text-center mb-16">
            <span className="pill-badge mb-6">Private Testnet Metrics</span>
            <h2 className="font-display text-4xl font-bold sm:text-5xl mb-4">
              Numbers that <span className="gradient-text">matter</span>
            </h2>
            <p className="text-muted-foreground">Clean-room engineering. Every line built from scratch.</p>
          </div>
        </ScrollSection>
        <div className="grid gap-8 sm:grid-cols-3">
          {STATS.map((stat, i) => (
            <ScrollSection key={stat.label} delay={i * 0.15}>
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
        <ScrollSection delay={0.4}>
          <div className="mt-16 flex justify-center">
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
      <div className="gradient-divider" />
    </section>
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
      <div className="h-16 flex items-center">{children(key)}</div>
      <p className="text-xs text-ink-low mt-3">{spec}</p>
    </div>
  );
}

function SystemReference() {
  const reduced = usePrefersReducedMotion();
  const ramp = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7", "n8", "n9"];
  const [statusOn, setStatusOn] = useState(true);
  return (
    <section className="bg-surface-0 border-t border-line-strong" style={{ padding: "var(--section-pad) 0" }}>
      <div className="mx-auto max-w-7xl px-6 sm:px-8 space-y-16">
        <SectionHeader kicker="System reference" title="Tokens on real words" />

        <div className="space-y-6">
          <p className="font-display text-display font-semibold text-ink-hi" style={{ letterSpacing: "-0.03em" }}>
            The chain proves itself.
          </p>
          <p className="font-display text-h2x font-semibold text-ink-hi">Numbers that matter</p>
          <p className="font-display text-h3x font-medium text-ink-hi">HotStuff-inspired BFT consensus</p>
          <p className="font-display text-stat font-light text-ink-hi tabular-nums">3,352,319</p>
          <p className="text-bodyx text-ink-mid max-w-xl">
            Body text carries most of the reading. It sits one emphasis level below headlines and stays fully
            readable on every ground it is allowed to sit on.
          </p>
          <MonoLabel>Uppercase mono label, plus five percent tracking</MonoLabel>
        </div>

        <div>
          <MonoLabel>Neutral ramp, hue drifts 228 to 210</MonoLabel>
          <div className="grid grid-cols-5 sm:grid-cols-10 gap-2 mt-4">
            {ramp.map((n) => (
              <div key={n} className="space-y-2">
                <div className="h-14 rounded-md border border-line-subtle" style={{ background: `hsl(var(--${n}))` }} />
                <p className="text-xs text-ink-low font-mono">{n}</p>
              </div>
            ))}
          </div>
        </div>

        <div>
          <MonoLabel>Text emphasis on allowed grounds</MonoLabel>
          <div className="grid sm:grid-cols-2 gap-4 mt-4">
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
        </div>

        <div>
          <MonoLabel>Accents and their scope, enforced by design-rules test</MonoLabel>
          <div className="grid sm:grid-cols-3 gap-4 mt-4">
            <div className="border border-line rounded-lg p-5 bg-surface-1 space-y-3">
              <button className="rounded-md px-4 py-2 text-sm font-semibold text-white" style={{ background: "hsl(var(--brand))", boxShadow: "var(--glow-2)" }}>
                Primary action
              </button>
              <p className="text-sm">
                <a href="#top" className="text-brand-text hover:underline">brand-text link on dark ground</a>
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
                live (cyan): live-state only, one element per viewport. Click the dot: status-transition, 0.3s
                crossfade, instant swap under reduced motion.
              </p>
            </div>
            <div className="border border-line rounded-lg p-5 bg-surface-1 space-y-3">
              <p className="font-display text-h3x font-semibold">
                A new beginning for{" "}
                <span className="gradient-text">intelligence</span>
              </p>
              <p className="text-xs text-ink-low">violet: third gradient stop only, one gradient per section, zero in technical sections</p>
            </div>
          </div>
        </div>

        <div>
          <MonoLabel>Motion primitives (system honors reduced motion: {reduced ? "ON, reduced states shown" : "off"})</MonoLabel>
          <div className="grid sm:grid-cols-3 gap-4 mt-4">
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
        </div>

        <div>
          <MonoLabel>Glow, three capped levels</MonoLabel>
          <div className="grid sm:grid-cols-3 gap-4 mt-4">
            {(["--glow-1", "--glow-2", "--glow-3"] as const).map((g) => (
              <div key={g} className="h-20 rounded-lg border border-line bg-surface-1 flex items-center justify-center" style={{ boxShadow: `var(${g})` }}>
                <p className="text-xs text-ink-low font-mono">{g}</p>
              </div>
            ))}
          </div>
          <p className="text-xs text-ink-low mt-3">glow-3 is hero and commit-flash only. Cards never exceed glow-2. The old triple-100px stack is deleted.</p>
        </div>
      </div>
    </section>
  );
}

export default function SpecimenApp() {
  return (
    <LazyMotion features={domAnimation}>
      <div id="top" className="min-h-screen bg-surface-0 text-ink-hi antialiased">
        <header className="border-b border-line-strong px-6 py-4">
          <p className="font-display text-h3x font-semibold text-ink-hi">Design specimen: stats section</p>
          <p className="text-sm text-ink-mid mt-1">
            Dev-only page. The same section, same derived data, rendered in system v2 (top) and the current
            production treatment (below). Not linked from the site, absent from the production build.
          </p>
        </header>
        <PaneLabel>Proposed: system v2</PaneLabel>
        <ProposedStats />
        <PaneLabel>Current production treatment</PaneLabel>
        <CurrentStats />
        <SystemReference />
      </div>
    </LazyMotion>
  );
}
