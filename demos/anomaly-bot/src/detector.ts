/**
 * Three simple anomaly heuristics.
 *
 * The detector is stateless w.r.t. cooldowns — main keeps a per-kind
 * "last-fired-at-height" map and skips re-firing within a window.
 */

interface BlockSnapshot {
  height: number;
  round: number;
  tx_count: number;
  observedAt: number; // ms epoch
}

export type AnomalyKind = "empty-streak" | "stalled" | "leader-rotation";

export interface Anomaly {
  kind: AnomalyKind;
  height: number;
  detail: Record<string, unknown>;
}

export const EMPTY_STREAK_THRESHOLD = 30;
export const STALL_THRESHOLD_MS = 15_000;
export const ROUND_HIGH_WINDOW = 10;

/**
 * Inspect a sliding window of recent block snapshots and a single "latest"
 * sample (for staleness detection). Returns the first anomaly found, or null.
 */
export function detect(
  window: BlockSnapshot[],
  latestObservedAt: number,
): Anomaly | null {
  if (window.length === 0) return null;
  const head = window[0]; // freshest

  // 1. Empty-block streak: every block in the window has tx_count = 0.
  if (
    window.length >= EMPTY_STREAK_THRESHOLD &&
    window.slice(0, EMPTY_STREAK_THRESHOLD).every((b) => b.tx_count === 0)
  ) {
    return {
      kind: "empty-streak",
      height: head.height,
      detail: {
        streak: EMPTY_STREAK_THRESHOLD,
        first_empty_height: window[EMPTY_STREAK_THRESHOLD - 1].height,
      },
    };
  }

  // 2. Stalled: head hasn't advanced in STALL_THRESHOLD_MS.
  const stalledFor = Date.now() - latestObservedAt;
  if (stalledFor > STALL_THRESHOLD_MS) {
    return {
      kind: "stalled",
      height: head.height,
      detail: { stalled_ms: stalledFor },
    };
  }

  // 3. Leader-rotation: any of the last ROUND_HIGH_WINDOW blocks has round > 0.
  const recent = window.slice(0, ROUND_HIGH_WINDOW);
  const highRound = recent.find((b) => b.round > 0);
  if (highRound) {
    return {
      kind: "leader-rotation",
      height: head.height,
      detail: { at_height: highRound.height, round: highRound.round },
    };
  }

  return null;
}
