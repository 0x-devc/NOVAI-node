#!/usr/bin/env node
// Contrast audit: computes WCAG 2.x contrast ratios for every text token
// against every ground it is allowed to sit on, straight from the token
// definitions in src/index.css. Content-carrying pairs must reach 4.5:1;
// a failure exits non-zero. Decorative and diagnostic pairs are printed for
// the record but do not gate.

import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "src", "index.css"), "utf8");

function token(name) {
  const re = new RegExp(`--${name}:\\s*([0-9.]+)\\s+([0-9.]+)%\\s+([0-9.]+)%`);
  const m = css.match(re);
  if (!m) throw new Error(`token --${name} not found as an HSL triplet in index.css`);
  return { h: Number(m[1]), s: Number(m[2]) / 100, l: Number(m[3]) / 100 };
}

function hslToRgb(c) {
  // A composited colour arrives as rgb already; everything else is HSL.
  if (c.rgb) return c.rgb;
  return hslFromTriplet(c);
}

function hslFromTriplet({ h, s, l }) {
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const hp = (((h % 360) + 360) % 360) / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let [r, g, b] = [0, 0, 0];
  if (hp < 1) [r, g, b] = [c, x, 0];
  else if (hp < 2) [r, g, b] = [x, c, 0];
  else if (hp < 3) [r, g, b] = [0, c, x];
  else if (hp < 4) [r, g, b] = [0, x, c];
  else if (hp < 5) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  const m = l - c / 2;
  return [r + m, g + m, b + m];
}

function luminance(rgb) {
  const [r, g, b] = rgb.map((v) => (v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a, b) {
  const [l1, l2] = [luminance(hslToRgb(a)), luminance(hslToRgb(b))].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
}

const WHITE = { h: 0, s: 0, l: 1 };
const t = {
  n0: token("n0"), n1: token("n1"), n2: token("n2"), n3: token("n3"),
  hi: token("n9"), mid: token("n8"), low: token("n7"), faint: token("n6"),
  brand: token("brand"), brandText: token("brand-text"), live: token("live"),
  warn: token("warn"), warnText: token("warn-text"),
  error: token("error"), errorText: token("error-text"),
  legacyMuted: token("muted-foreground"), legacyBg: token("background"), legacyCard: token("card"),
};

// The one colour the syntax tokens add that is not already in the palette.
// Declared here as well as in console.css because this audit reads index.css,
// and a token that lives only in a component stylesheet would go unchecked.
const TOK_STRING = { h: 145, s: 0.45, l: 0.62 };

/**
 * A foreground drawn at partial alpha over a known background, as the browser
 * composites it. Auditing the un-composited colour would report a contrast the
 * reader never sees.
 */
function composite(fg, bg, alpha) {
  const f = hslFromTriplet(fg);
  const b = hslFromTriplet(bg);
  return { rgb: f.map((v, i) => v * alpha + b[i] * (1 - alpha)) };
}

// [label, fg, bg, threshold, gating]
const pairs = [
  ["text-hi on n0 (page)", t.hi, t.n0, 4.5, true],
  ["text-hi on n1 (raised)", t.hi, t.n1, 4.5, true],
  ["text-hi on n2 (card)", t.hi, t.n2, 4.5, true],
  ["text-hi on n3 (surface-2)", t.hi, t.n3, 4.5, true],
  ["text-mid on n0", t.mid, t.n0, 4.5, true],
  ["text-mid on n1", t.mid, t.n1, 4.5, true],
  ["text-mid on n2", t.mid, t.n2, 4.5, true],
  ["text-mid on n3", t.mid, t.n3, 4.5, true],
  ["text-low on n0", t.low, t.n0, 4.5, true],
  ["text-low on n1", t.low, t.n1, 4.5, true],
  ["text-low on n2", t.low, t.n2, 4.5, true],
  ["text-low on n3", t.low, t.n3, 4.5, true],
  ["brand-text link on n0", t.brandText, t.n0, 4.5, true],
  ["brand-text link on n1", t.brandText, t.n1, 4.5, true],
  ["live (cyan) digits on n0", t.live, t.n0, 4.5, true],
  ["live (cyan) digits on n1", t.live, t.n1, 4.5, true],
  ["warn-text on n1", t.warnText, t.n1, 4.5, true],
  ["error-text on n1", t.errorText, t.n1, 4.5, true],
  ["white on brand (button)", WHITE, t.brand, 4.5, true],
  // SYNTAX TOKENS. Every one of these is CONTENT, not decoration: a key, a
  // value, a placeholder the reader has to substitute, and a comment that
  // explains what a field means. They are gated at 4.5 like body text.
  //
  // The code-block background is n1 for console-pre and n0 for console-pre-out,
  // so both are checked. Punctuation is dimmed by TIER rather than by alpha:
  // ethereum.org dims its most frequent token to 47 percent, and transferred to
  // this palette that measures 2.47, below even the non-text threshold. The
  // technique was right and the value was not transferable, so punctuation
  // takes text-low at full opacity, which is 5.60 where keys are 10.56.
  ["tok-string on n1 (code bg)", TOK_STRING, t.n1, 4.5, true],
  ["tok-string on n0 (response bg)", TOK_STRING, t.n0, 4.5, true],
  ["tok-prop = text-mid on n1", t.mid, t.n1, 4.5, true],
  ["tok-num = warn-text on n0", t.warnText, t.n0, 4.5, true],
  ["tok-comment = text-low on n1", t.low, t.n1, 4.5, true],
  ["tok-comment = text-low on n0", t.low, t.n0, 4.5, true],
  ["tok-type = brand-text on n1", t.brandText, t.n1, 4.5, true],
  ["tok-punct = text-low on n1", t.low, t.n1, 4.5, true],
  ["tok-punct = text-low on n0", t.low, t.n0, 4.5, true],
  // The rejected option, kept as a measurement rather than a memory:
  // ethereum.org dims punctuation with alpha, and transferred to this palette
  // that lands at 2.47, below even the non-text threshold. See console.css.
  ["REJECTED tok-punct alpha 55% on n1", composite(t.low, t.n1, 0.55), t.n1, 4.5, false],
  ["DECORATIVE text-faint on n0", t.faint, t.n0, 4.5, false],
  ["DECORATIVE text-faint on n2", t.faint, t.n2, 4.5, false],
  ["DIAGNOSTIC brand fill as text on n0", t.brand, t.n0, 4.5, false],
  ["DIAGNOSTIC warn fill as text on n1", t.warn, t.n1, 4.5, false],
  ["DIAGNOSTIC error fill as text on n1", t.error, t.n1, 4.5, false],
  ["LEGACY muted-foreground on background", t.legacyMuted, t.legacyBg, 4.5, false],
  ["LEGACY muted-foreground on card", t.legacyMuted, t.legacyCard, 4.5, false],
];

// Self-test: prove the meter before trusting its verdicts. White on black is
// 21:1 by definition; #767676 on white is the canonical 4.54 AA-boundary gray.
const BLACK = { h: 0, s: 0, l: 0 };
const BOUNDARY_GRAY = { h: 0, s: 0, l: 0.4627 }; // #767676
const anchorWB = ratio(WHITE, BLACK);
const anchorGray = ratio(BOUNDARY_GRAY, WHITE);
if (Math.abs(anchorWB - 21) > 0.01 || Math.abs(anchorGray - 4.54) > 0.02) {
  console.error(`self-test FAILED: white/black=${anchorWB.toFixed(3)} (needs 21.000), gray/white=${anchorGray.toFixed(3)} (needs 4.54)`);
  process.exit(1);
}
console.log(`self-test: ok (white/black = ${anchorWB.toFixed(3)}, #767676/white = ${anchorGray.toFixed(3)})\n`);

// Worked computation for the record: every intermediate value for one passing
// content pair and the deliberately failing decorative token.
function explain(label, fg, bg) {
  const chain = (c) => {
    const rgb = hslToRgb(c);
    const lin = rgb.map((v) => (v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)));
    const lum = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
    return { rgb, lin, lum };
  };
  const F = chain(fg), B = chain(bg);
  const hex = (rgb) => "#" + rgb.map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("");
  console.log(`worked: ${label}`);
  console.log(`  fg hsl(${fg.h} ${fg.s * 100}% ${fg.l * 100}%) -> ${hex(F.rgb)} -> sRGB [${F.rgb.map((v) => v.toFixed(4)).join(", ")}]`);
  console.log(`     linearized [${F.lin.map((v) => v.toFixed(5)).join(", ")}] -> luminance ${F.lum.toFixed(5)}`);
  console.log(`  bg hsl(${bg.h} ${bg.s * 100}% ${bg.l * 100}%) -> ${hex(B.rgb)} -> sRGB [${B.rgb.map((v) => v.toFixed(4)).join(", ")}]`);
  console.log(`     linearized [${B.lin.map((v) => v.toFixed(5)).join(", ")}] -> luminance ${B.lum.toFixed(5)}`);
  const [l1, l2] = [F.lum, B.lum].sort((a, b) => b - a);
  console.log(`  ratio = (${l1.toFixed(5)} + 0.05) / (${l2.toFixed(5)} + 0.05) = ${((l1 + 0.05) / (l2 + 0.05)).toFixed(2)}\n`);
}
explain("text-low on n0 (weakest passing content pair)", t.low, t.n0);
explain("text-faint on n0 (the decorative-only failure)", t.faint, t.n0);

let failures = 0;
console.log("pair".padEnd(42) + "ratio".padStart(8) + "  needs  verdict");
for (const [label, fg, bg, min, gating] of pairs) {
  const r = ratio(fg, bg);
  const pass = r >= min;
  if (gating && !pass) failures += 1;
  const verdict = gating ? (pass ? "PASS" : "FAIL") : pass ? "(passes)" : "(below, non-gating)";
  console.log(label.padEnd(42) + r.toFixed(2).padStart(8) + `  ${min}    ${verdict}`);
}
console.log(`\ngating failures: ${failures}`);
process.exit(failures > 0 ? 1 : 0);
