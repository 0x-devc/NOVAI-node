// Presentation-layer rounding for derived repo metrics: floor to a step and
// suffix "+" so a displayed claim stays true as the tree grows. The generated
// JSON keeps exact values; only display floors.
export function floorTo(value: number, step: number): number {
  return Math.floor(value / step) * step;
}

export function flooredClaim(value: number, step: number): string {
  return `${floorTo(value, step).toLocaleString("en-US")}+`;
}

/** Compact age string from seconds: "42s", "3m 24s", "1h 12m". */
export function fmtAge(seconds: number): string {
  if (seconds < 60) return `${Math.floor(seconds)}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m ${Math.floor(seconds % 60)}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}
