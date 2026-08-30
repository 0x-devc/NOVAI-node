import { useRef, useState } from "react";
import { LazyMotion, domAnimation, m, motion } from "framer-motion";
import { Code2 } from "lucide-react";
import AnimatedCounter from "@/components/novai/AnimatedCounter";
import ScrollSection from "@/components/novai/ScrollSection";
import SectionHeader from "@/components/system/SectionHeader";
import StatTile from "@/components/system/StatTile";
import MonoLabel from "@/components/system/MonoLabel";
import Reveal from "@/components/system/Reveal";
import LiveChainStats from "@/components/system/LiveChainStats";
import VerifyPanel from "@/components/console/VerifyPanel";
import Panel, { Caption, PanelRow } from "@/components/console/Panel";
import { STATS } from "@/data/stats";
import repoStats from "@/data/repo-stats.generated.json";
import chainSnapshot from "@/data/chain-snapshot.json";
import { floorTo } from "@/lib/format";
import { fadeRise, statSettle, statusTransition } from "@/lib/motion";
import { usePrefersReducedMotion } from "@/hooks/usePrefersReducedMotion";
import { useChainStatus, type ChainStatus } from "@/hooks/useChainStatus";
import { useInView } from "@/hooks/useInView";

// Dev-only judging console. Dense on purpose: data is the content here.
// The 01 network panel is the prototype for the site's #network section.
// Served by the dev server only; absent from the production build.

const TESTS_CLAIM = `${floorTo(repoStats.tests.value, 100).toLocaleString()}+`;

const NAV = [
  ["01", "live", "network"],
  ["02", "verify", "verify"],
  ["03", "compare", "compare"],
  ["04", "tokens", "tokens"],
  ["05", "type", "type"],
  ["06", "accents", "accents"],
  ["07", "motion", "motion"],
  ["08", "glow", "glow"],
] as const;

function SectionHead({ num, title }: { num: string; title: string }) {
  return (
    <div className="flex items-center gap-3 pt-8 pb-3">
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-faint">{num}</span>
      <span className="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-low">{title}</span>
      <span className="flex-1 border-t border-line-subtle" />
    </div>
  );
}

type ForcedState = "auto" | "loading" | "live" | "stale" | "unreachable";

function forcedStatus(kind: Exclude<ForcedState, "auto">, real: ChainStatus): ChainStatus {
  const snapshot = real.snapshot;
  const base = { snapshot, txCount: 0, failures: 0, blockHash: snapshot.blockHash, stateRoot: snapshot.stateRoot };
  switch (kind) {
    case "loading":
      return { ...base, state: "loading", height: null, round: null, bps: null, ageSeconds: null, blockHash: null, stateRoot: null };
    case "live":
      return {
        ...base,
        state: "live",
        height: real.height ?? snapshot.height,
        round: real.round ?? snapshot.round,
        txCount: real.txCount ?? snapshot.txCount,
        bps: real.bps ?? 1.12,
        ageSeconds: real.ageSeconds ?? 2,
      };
    case "stale":
      return { ...base, state: "stale", height: real.height ?? snapshot.height, round: 0, bps: null, ageSeconds: 754 };
    case "unreachable":
      return { ...base, state: "unreachable", height: null, round: null, bps: null, ageSeconds: null, failures: 5 };
  }
}

function NetworkSection() {
  const ref = useRef<HTMLDivElement>(null);
  const inView = useInView(ref);
  const real = useChainStatus({ rpcUrl: "/rpc", fast: inView });
  const [forced, setForced] = useState<ForcedState>("auto");
  const shown = forced === "auto" ? real : forcedStatus(forced, real);
  return (
    <div ref={ref} className="space-y-2">
      <LiveChainStats
        status={shown}
        meta={
          <span className="flex items-center gap-1">
            {(["auto", "loading", "live", "stale", "unreachable"] as const).map((k) => (
              <button
                key={k}
                onClick={() => setForced(k)}
                className={`rounded px-1.5 py-0.5 text-[10px] font-mono ${
                  forced === k ? "bg-surface-3 text-ink-hi" : "text-ink-low hover:text-ink-mid"
                }`}
              >
                {k}
              </button>
            ))}
            {forced !== "auto" && (
              <span className="font-mono text-[10px] uppercase text-warnx-text ml-1">forced</span>
            )}
          </span>
        }
      />
      <p className="text-[11px] text-ink-low">
        auto = the real chain through the dev proxy (localhost /rpc to rpc.novai.network; dev server only).
        Poll cadence: 2s while this panel is on screen, 10s off screen, paused when the tab hides.
      </p>
    </div>
  );
}

function CompareSection() {
  return (
    <div className="space-y-4">
      <div>
        <MonoLabel>marketing register, system v2 (for narrative sections)</MonoLabel>
        <div className="mt-2 border border-line-subtle rounded-md bg-surface-0 px-8 py-10">
          <Reveal>
            <SectionHeader
              kicker="Private testnet"
              title="Numbers that matter"
              lede="Clean-room engineering. Every line built from scratch."
            />
          </Reveal>
          <Reveal className="mt-8">
            <div className="grid sm:grid-cols-3 divide-y sm:divide-y-0 sm:divide-x divide-line-subtle border-y border-line-subtle">
              <StatTile value="Active" label="Private testnet running" provenance="static status" />
              <StatTile value="4" label="Validators" provenance="fleet configuration" />
              <StatTile value={TESTS_CLAIM} label="Tests" provenance="counted from source at build" />
            </div>
          </Reveal>
        </div>
      </div>
      <div>
        <MonoLabel>current production treatment (being replaced)</MonoLabel>
        <div className="mt-2 border border-line-subtle rounded-md overflow-hidden">
          <div className="relative py-12" style={{ background: "hsl(228 30% 4%)" }}>
            <div className="section-container relative z-10">
              <ScrollSection>
                <div className="text-center mb-10">
                  <span className="pill-badge mb-5">Private Testnet Metrics</span>
                  <h2 className="font-display text-4xl font-bold mb-3">
                    Numbers that <span className="gradient-text">matter</span>
                  </h2>
                  <p className="text-muted-foreground">Clean-room engineering. Every line built from scratch.</p>
                </div>
              </ScrollSection>
              <div className="grid gap-8 sm:grid-cols-3">
                {STATS.map((stat) => (
                  <ScrollSection key={stat.label}>
                    <div className="text-center">
                      <div className="font-display text-5xl font-bold gradient-text mb-2">
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
                <div className="mt-10 flex justify-center">
                  <div className="glass-card rounded-2xl px-8 py-5 flex items-center gap-4">
                    <Code2 size={24} className="text-accent" />
                    <div>
                      <p className="font-display text-lg font-semibold text-foreground">Built from Scratch in Rust</p>
                      <p className="text-xs text-muted-foreground">No forks. No copied code.</p>
                    </div>
                  </div>
                </div>
              </ScrollSection>
            </div>
          </div>
        </div>
      </div>
      <p className="text-[11px] text-ink-low">
        Two registers, one system: StatTile carries narrative sections; the console vocabulary (01, 02) carries
        data surfaces like #network.
      </p>
    </div>
  );
}

function TokensSection() {
  const ramp = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7", "n8", "n9"];
  return (
    <Panel title="neutral ramp and text emphasis">
      <PanelRow>
        <div className="grid grid-cols-10 gap-1">
          {ramp.map((n) => (
            <div key={n}>
              <div className="h-8 rounded-sm border border-line-subtle" style={{ background: `hsl(var(--${n}))` }} />
              <p className="text-[10px] text-ink-low font-mono mt-0.5">{n}</p>
            </div>
          ))}
        </div>
      </PanelRow>
      <div className="grid sm:grid-cols-2 divide-y sm:divide-y-0 sm:divide-x divide-line-subtle">
        {(["surface-0", "surface-2"] as const).map((ground) => (
          <div key={ground} className={`bg-${ground} px-4 py-3 space-y-1`}>
            <p className="text-ink-hi text-sm">text-hi: headlines and values</p>
            <p className="text-ink-mid text-sm">text-mid: body copy</p>
            <p className="text-ink-low text-sm">text-low: labels and provenance</p>
            <p className="text-ink-faint text-sm">
              text-faint: decorative only <span className="text-errorx-text">(barred from content)</span>
            </p>
            <p className="text-[10px] text-ink-low font-mono pt-1">on {ground}</p>
          </div>
        ))}
      </div>
      <Caption>Measured ratios live in the contrast audit; content pairs hold 4.5 or better on every allowed ground.</Caption>
    </Panel>
  );
}

function TypeSection() {
  return (
    <Panel title="type on real words">
      <PanelRow>
        <p className="font-display text-display font-semibold text-ink-hi">The chain proves itself.</p>
        <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1">
          display, clamp 40 to 88px, grotesk 600, tracking -3 percent, 23ch cap
        </p>
      </PanelRow>
      <PanelRow>
        <p className="font-display text-h2x font-semibold text-ink-hi">Numbers that matter</p>
        <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1">h2, clamp 28 to 56px</p>
      </PanelRow>
      <PanelRow>
        <p className="font-display text-h3x font-medium text-ink-hi">HotStuff-inspired BFT consensus</p>
        <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1">h3, clamp 20 to 28px</p>
      </PanelRow>
      <PanelRow>
        <p className="font-mono text-stat font-light text-ink-hi tabular-nums">{chainSnapshot.height.toLocaleString()}</p>
        <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1">
          stat, mono for live digits, tabular numerals
        </p>
      </PanelRow>
      <PanelRow>
        <p className="text-bodyx text-ink-mid max-w-xl">
          Body text carries most of the reading. It sits one emphasis level below headlines and stays fully
          readable on every ground it is allowed to sit on.
        </p>
        <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low mt-1">body, 16 to 18px, lh 1.6</p>
      </PanelRow>
    </Panel>
  );
}

function AccentsSection() {
  const reduced = usePrefersReducedMotion();
  const [statusOn, setStatusOn] = useState(true);
  return (
    <Panel title="accents and their scope, enforced by design-rules tests">
      <PanelRow className="flex flex-wrap items-center gap-4">
        <button
          className="rounded-md px-3 py-1.5 text-sm font-semibold text-white"
          style={{ background: "hsl(var(--brand))", boxShadow: "var(--glow-2)" }}
        >
          Primary action
        </button>
        <a href="#live" className="text-sm text-brand-text hover:underline">
          brand-text link
        </a>
        <span className="text-[11px] text-ink-low">brand: interactive fills and links only</span>
      </PanelRow>
      <PanelRow className="flex flex-wrap items-center gap-4">
        <button onClick={() => setStatusOn((v) => !v)} className="flex items-center gap-2">
          <m.span
            animate={{ backgroundColor: statusOn ? "hsl(192 95% 68%)" : "hsl(38 92% 58%)" }}
            transition={statusTransition(reduced)}
            className="inline-block h-2.5 w-2.5 rounded-full"
          />
          <span className="font-mono text-[11px] uppercase tracking-[0.05em] text-ink-mid">
            {statusOn ? "live" : "paused"}
          </span>
        </button>
        <span className="text-[11px] text-ink-low">
          live cyan: live-state only, one element per viewport, click to see status-transition
        </span>
      </PanelRow>
      <PanelRow className="flex flex-wrap items-center gap-4">
        <span className="font-display text-h3x font-semibold">
          A new beginning for <span className="gradient-text">intelligence</span>
        </span>
        <span className="text-[11px] text-ink-low">violet: third gradient stop only, zero in quiet sections</span>
      </PanelRow>
    </Panel>
  );
}

function MotionDemoRow({ name, spec, children }: { name: string; spec: string; children: (key: number) => React.ReactNode }) {
  const [key, setKey] = useState(0);
  return (
    <PanelRow className="flex flex-wrap items-center gap-4">
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-low w-24">{name}</span>
      <div className="h-10 flex items-center flex-1 min-w-40">{children(key)}</div>
      <button onClick={() => setKey((k) => k + 1)} className="text-xs font-mono text-brand-text hover:underline">
        replay
      </button>
      <span className="text-[11px] text-ink-low basis-full sm:basis-auto">{spec}</span>
    </PanelRow>
  );
}

function MotionSection() {
  const reduced = usePrefersReducedMotion();
  return (
    <Panel title="motion primitives" meta={<span className="font-mono text-[10px] text-ink-low">reduced motion: {reduced ? "on" : "off"}</span>}>
      <MotionDemoRow name="fade-rise" spec="0.5s ease-out-quart, y 16px. Reduced: static, opaque, no transform.">
        {(key) => (
          <m.div key={key} initial="hidden" animate="visible" variants={reduced ? fadeRise.reduced : fadeRise.full} className="text-sm text-ink-mid">
            Section content reveal
          </m.div>
        )}
      </MotionDemoRow>
      <MotionDemoRow name="stat-settle" spec="0.3s opacity + blur. Never counts from zero. Reduced: final value, no ramp.">
        {(key) => (
          <m.div key={key} initial="hidden" animate="visible" variants={reduced ? statSettle.reduced : statSettle.full} className="font-mono text-xl font-light text-ink-hi tabular-nums">
            {TESTS_CLAIM}
          </m.div>
        )}
      </MotionDemoRow>
      <MotionDemoRow name="hover-lift" spec="150ms, y -2px + border. Reduced: color only, no movement.">
        {() => (
          <div className="border border-line rounded px-3 py-1 text-sm text-ink-mid transition-all duration-150 hover:-translate-y-0.5 hover:border-line-strong cursor-default">
            hover me
          </div>
        )}
      </MotionDemoRow>
    </Panel>
  );
}

function GlowSection() {
  return (
    <Panel title="glow, three capped levels">
      <PanelRow className="grid sm:grid-cols-3 gap-4">
        {(["--glow-1", "--glow-2", "--glow-3"] as const).map((g) => (
          <div key={g} className="h-14 rounded-md border border-line bg-surface-1 flex items-center justify-center" style={{ boxShadow: `var(${g})` }}>
            <span className="text-[10px] text-ink-low font-mono">{g}</span>
          </div>
        ))}
      </PanelRow>
      <Caption>glow-3 is hero and commit-flash only. Cards never exceed glow-2.</Caption>
    </Panel>
  );
}

export default function SpecimenApp() {
  return (
    <LazyMotion features={domAnimation}>
      <div className="min-h-screen bg-surface-0 text-ink-hi antialiased">
        <header className="border-b border-line-strong">
          <div className="mx-auto max-w-7xl px-5 py-3 flex items-baseline justify-between">
            <p className="font-display text-base font-semibold text-ink-hi">NOVAI design console</p>
            <p className="text-[11px] text-ink-low">dev only, not linked, not in the production build</p>
          </div>
        </header>
        <nav className="xl:hidden sticky top-0 z-nav bg-surface-0/90 backdrop-blur-sm border-b border-line-strong">
          <div className="mx-auto max-w-7xl px-5 py-2 flex flex-wrap gap-x-4 gap-y-1">
            {NAV.map(([num, id, label]) => (
              <a key={id} href={`#${id}`} className="text-[11px] font-mono text-ink-low hover:text-ink-hi no-underline">
                <span className="text-ink-faint mr-1">{num}</span>
                {label}
              </a>
            ))}
          </div>
        </nav>

        <div className="mx-auto max-w-7xl px-5 xl:grid xl:grid-cols-[9rem_1fr] xl:gap-8">
          <aside className="hidden xl:block">
            <nav className="sticky top-6 py-8 space-y-2">
              {NAV.map(([num, id, label]) => (
                <a key={id} href={`#${id}`} className="block text-[11px] font-mono text-ink-low hover:text-ink-hi no-underline">
                  <span className="text-ink-faint mr-1.5">{num}</span>
                  {label}
                </a>
              ))}
            </nav>
          </aside>

          <main className="pb-16 min-w-0">
            <section id="live" className="scroll-mt-12">
              <SectionHead num="01" title="network console (prototype for #network)" />
              <NetworkSection />
            </section>
            <section id="verify" className="scroll-mt-12">
              <SectionHead num="02" title="verify panel (gate 4)" />
              <VerifyPanel rpcUrl="/rpc" />
            </section>
            <section id="compare" className="scroll-mt-12">
              <SectionHead num="03" title="stats section, both registers" />
              <CompareSection />
            </section>
            <section id="tokens" className="scroll-mt-12">
              <SectionHead num="04" title="tokens" />
              <TokensSection />
            </section>
            <section id="type" className="scroll-mt-12">
              <SectionHead num="05" title="type" />
              <TypeSection />
            </section>
            <section id="accents" className="scroll-mt-12">
              <SectionHead num="06" title="accents" />
              <AccentsSection />
            </section>
            <section id="motion" className="scroll-mt-12">
              <SectionHead num="07" title="motion" />
              <MotionSection />
            </section>
            <section id="glow" className="scroll-mt-12">
              <SectionHead num="08" title="glow" />
              <GlowSection />
            </section>
          </main>
        </div>
      </div>
    </LazyMotion>
  );
}
