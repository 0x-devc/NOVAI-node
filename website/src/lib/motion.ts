// Motion primitives for the v2 design system. Every primitive carries an
// explicit `reduced` variant: the CONCRETE final state rendered when the
// visitor prefers reduced motion, not a vague "freeze".
import type { Variants, Transition } from "framer-motion";

export const EASE_OUT_EXPO = [0.16, 1, 0.3, 1] as const;
export const EASE_OUT_QUART = [0.22, 1, 0.36, 1] as const;
export const EASE_DIGIT = [0.65, 0, 0.35, 1] as const;

export const DURATION = {
  micro: 0.15,
  state: 0.3,
  reveal: 0.5,
  hero: 0.7,
} as const;

// fade-rise: in-view once. Reduced: static at final position, fully opaque,
// zero transform, zero delay (content readable on first paint).
export const fadeRise: { full: Variants; reduced: Variants } = {
  full: {
    hidden: { opacity: 0, y: 16 },
    visible: { opacity: 1, y: 0, transition: { duration: DURATION.reveal, ease: EASE_OUT_QUART } },
  },
  reduced: {
    hidden: { opacity: 1, y: 0 },
    visible: { opacity: 1, y: 0 },
  },
};

// fade: in-view once, opacity only. Reduced: rendered opaque, no tween.
export const fade: { full: Variants; reduced: Variants } = {
  full: {
    hidden: { opacity: 0 },
    visible: { opacity: 1, transition: { duration: 0.4, ease: "easeOut" } },
  },
  reduced: {
    hidden: { opacity: 1 },
    visible: { opacity: 1 },
  },
};

// stat-settle: values NEVER count from zero (the server-rendered markup holds
// the final number); the full state is a 0.3s opacity+blur settle. Reduced:
// final value, no blur, no opacity ramp.
export const statSettle: { full: Variants; reduced: Variants } = {
  full: {
    hidden: { opacity: 0, filter: "blur(4px)" },
    visible: { opacity: 1, filter: "blur(0px)", transition: { duration: DURATION.state, ease: "easeOut" } },
  },
  reduced: {
    hidden: { opacity: 1, filter: "blur(0px)" },
    visible: { opacity: 1, filter: "blur(0px)" },
  },
};

// stagger-children: 60ms per child, capped at 5 children then grouped.
// Reduced: zero stagger, all children at final state simultaneously.
export const staggerChildren = (reduced: boolean): Transition =>
  reduced ? {} : { staggerChildren: 0.06 };

// hero-intro: mount-once, 0.7s, 80ms stagger, max 4 elements. Reduced: all
// elements at final opacity and position on first paint, no delay chain.
export const heroIntro: { full: Variants; reduced: Variants } = {
  full: {
    hidden: { opacity: 0, y: 24 },
    visible: { opacity: 1, y: 0, transition: { duration: DURATION.hero, ease: EASE_OUT_EXPO } },
  },
  reduced: {
    hidden: { opacity: 1, y: 0 },
    visible: { opacity: 1, y: 0 },
  },
};

// status-transition (live/stale/snapshot dot and copy): 0.3s color crossfade.
// Reduced: instant swap (duration 0), same colors.
export const statusTransition = (reduced: boolean): Transition =>
  reduced ? { duration: 0 } : { duration: DURATION.state, ease: "easeInOut" };

// odometer-tick (Gate 5): 0.4s translateY digit roll on the last four digits.
// Reduced: plain text swap, no roll, still updates (PRM users keep live data).
// commit-pulse (Gate 5): 0.6s glow swell on height change. Reduced: none, the
// glow level stays constant at glow-2.
// hover-lift (CSS only): 150ms translateY(-2px) + border-color. Reduced: the
// global CSS floor collapses the transition; color change only, no movement.
