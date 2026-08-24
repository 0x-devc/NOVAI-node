// Presentation-layer rounding for derived repo metrics: floor to a step and
// suffix "+" so a displayed claim stays true as the tree grows. The generated
// JSON keeps exact values; only display floors.
export function floorTo(value: number, step: number): number {
  return Math.floor(value / step) * step;
}

export function flooredClaim(value: number, step: number): string {
  return `${floorTo(value, step).toLocaleString("en-US")}+`;
}
