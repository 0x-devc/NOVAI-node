/** Truncate a long hex string for display (8 + … + 8). */
export function shortHex(hex: string, headLen = 8, tailLen = 8): string {
  if (hex.length <= headLen + tailLen + 1) return hex;
  return `${hex.slice(0, headLen)}…${hex.slice(-tailLen)}`;
}

/** Format a u128 decimal string with thousands separators. */
export function formatBigInt(value: string): string {
  if (!/^\d+$/.test(value)) return value;
  return value.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Format a u64 number with thousands separators. */
export function formatNumber(n: number): string {
  return n.toLocaleString("en-US");
}

/** Decode a hex-encoded byte string to UTF-8 (or back to hex if invalid). */
export function hexToUtf8(hex: string): string {
  if (!/^[0-9a-fA-F]*$/.test(hex) || hex.length % 2 !== 0) return hex;
  try {
    const bytes = new Uint8Array(hex.length / 2);
    for (let i = 0; i < bytes.length; i++) {
      bytes[i] = parseInt(hex.substring(i * 2, i * 2 + 2), 16);
    }
    const decoded = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
    // Reject if a lot of replacement characters made it through.
    const replacements = (decoded.match(/\uFFFD/g) ?? []).length;
    if (replacements > decoded.length / 4) return hex;
    return decoded;
  } catch {
    return hex;
  }
}

const SIGNAL_TYPES = [
  "anomaly",
  "optimization",
  "prediction",
  "risk-score",
  "audit-report",
  "spam-risk",
  "congestion-forecast",
];

export function signalTypeName(byte: number): string {
  return SIGNAL_TYPES[byte] ?? `unknown(${byte})`;
}

const MEMORY_TYPES = [
  "chain-summary",
  "label-index",
  "embedding-commitment",
  "anomaly-log",
  "statistics-snapshot",
];

export function memoryTypeName(byte: number): string {
  return MEMORY_TYPES[byte] ?? `unknown(${byte})`;
}

export function autonomyModeName(byte: number): string {
  return ["Advisory", "Gated", "Autonomous (reserved)"][byte] ?? `unknown(${byte})`;
}

export function capabilitiesList(byte: number): string[] {
  const flags: string[] = [];
  if (byte & 0x01) flags.push("read_chain");
  if (byte & 0x02) flags.push("read_memory");
  if (byte & 0x04) flags.push("emit_proposals");
  if (byte & 0x08) flags.push("request_execution");
  if (byte & 0x10) flags.push("read_nnpx");
  return flags;
}
