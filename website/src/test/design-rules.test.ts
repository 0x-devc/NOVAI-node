import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, resolve, relative } from "node:path";

// Design-system rules enforced mechanically so they cannot erode by Gate 6.
// Accent scarcity, faint-token scope, glow ceiling, and the contrast floor
// are tests, not review conventions.

function* walk(dir: string): Generator<string> {
  for (const name of readdirSync(dir).sort()) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else if (/\.(tsx?|css)$/.test(name)) yield p;
  }
}

const SRC = resolve("src");
const files = [...walk(SRC)]
  .map((p) => ({ rel: relative(resolve("."), p), text: readFileSync(p, "utf8") }))
  .filter((f) => !f.rel.startsWith("src/test/"));

// Legacy files are exempt until their Gate 5-6 rebuild replaces them; nothing
// may be ADDED to this list without a gate decision.
const LEGACY_EXEMPT = new Set([
  "src/pages/SinglePage.tsx",
  "src/index.css",
  "src/App.css",
  "src/components/novai",
  "src/components/ui",
]);
/**
 * Source with comments removed, for rules that police what a file DOES.
 *
 * These rules match token names as bare substrings, so a comment explaining why
 * a token was NOT used trips them: writing "text-low, not text-faint" in a CSS
 * comment reads to the scanner exactly like using text-faint. That is the same
 * defect as attaching a caveat by scanning an exception's prose, which this
 * gate's own codebase spent a release fixing, and the same one the Rust
 * dispatch scan already guards against by stripping comments "so a
 * commented-out arm cannot count".
 *
 * Block comments are removed for both CSS and TypeScript. Line comments are
 * removed only when the line STARTS with //, so a "https://" inside a string is
 * never mistaken for one. A declaration inside a comment is inert, so nothing
 * is weakened by this.
 */
const withoutComments = (text: string) =>
  text
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split("\n")
    .map((line) => (/^\s*\/\//.test(line) ? "" : line))
    .join("\n");

const isLegacy = (rel: string) => [...LEGACY_EXEMPT].some((e) => rel === e || rel.startsWith(e + "/"));

describe("design rules", () => {
  // The console is monochrome by construction rather than by discipline. Its
  // markup lives in console.html, which is static rather than React, plus
  // src/console/. The src walker covers neither .html nor anything outside
  // src, so both are gathered explicitly.
  //
  // Not mechanically checkable, and therefore still a review item: brand used
  // decoratively rather than on a link or control. Brand on links is permitted,
  // so a count cap here would fire on ordinary content.
  it("the console carries no marketing register", () => {
    const consoleFiles = [
      ...files.filter((f) => f.rel.startsWith("src/console/")),
      { rel: "console.html", text: readFileSync(resolve("console.html"), "utf8") },
    ];
    // A rule with nothing to police is not a rule.
    expect(consoleFiles.length, "expected console surfaces to exist").toBeGreaterThan(1);

    const FORBIDDEN: [RegExp, string][] = [
      [/gradient-text|bg-gradient|text-transparent/, "a gradient treatment"],
      [/violet|--violet/, "violet, which is a marketing accent"],
      [/glow-?[23]\b/, "a glow above glow-1"],
      [/(?:text|bg|border|fill|stroke|ring)-live\b|--live\b|hsl\(\s*192[,\s]/, "the live/cyan accent directly"],
      [/text-ink-faint|text-faint\b/, "the faint token, which fails contrast for content"],
    ];
    for (const f of consoleFiles) {
      const code = withoutComments(f.text);
      for (const [re, what] of FORBIDDEN) {
        expect(re.test(code), `${f.rel} uses ${what}`).toBe(false);
      }
    }
  });

  it("cyan (live token) appears only in live-state components and the specimen", () => {
    const ALLOW = new Set([
      "src/index.css", // token definition
      "src/dev/SpecimenApp.tsx", // reference demo
      "src/components/system/StatusDot.tsx", // the canonical live-state indicator
    ]);
    const cyanRe = /(?:text|bg|border|fill|stroke|ring)-live\b|--live\b|hsl\(\s*192[,\s]/;
    for (const f of files) {
      if (ALLOW.has(f.rel) || isLegacy(f.rel)) continue;
      expect(cyanRe.test(f.text), `${f.rel} uses the live/cyan accent outside the allowlist`).toBe(false);
    }
  });

  it("each future section file caps at one live-accent element", () => {
    const dir = resolve("src/sections");
    if (!existsSync(dir)) return; // rule armed for Gates 5-6
    for (const f of files.filter((x) => x.rel.startsWith("src/sections/"))) {
      const count = (f.text.match(/(?:text|bg|border|fill|stroke|ring)-live\b/g) ?? []).length;
      expect(count, `${f.rel} exceeds one live-accent element`).toBeLessThanOrEqual(1);
    }
  });

  it("gradient text stays out of quiet sections and appears at most once elsewhere", () => {
    const QUIET = ["Network", "Testnet", "Documents", "Contribute", "Socials"];
    for (const f of files.filter((x) => x.rel.startsWith("src/sections/"))) {
      const count = (f.text.match(/gradient-text/g) ?? []).length;
      const base = f.rel.replace(/^src\/sections\//, "").replace(/\.tsx$/, "");
      if (QUIET.includes(base)) expect(count, `${f.rel} uses gradient text in a quiet section`).toBe(0);
      else expect(count, `${f.rel} uses gradient text more than once`).toBeLessThanOrEqual(1);
    }
  });

  it("text-faint is decorative only: barred from new components", () => {
    const ALLOW = new Set(["src/index.css", "tailwind.config.ts", "src/dev/SpecimenApp.tsx"]);
    for (const f of files) {
      if (ALLOW.has(f.rel) || isLegacy(f.rel)) continue;
      expect(/text-ink-faint|text-faint\b/.test(withoutComments(f.text)), `${f.rel} puts content on the faint token`).toBe(false);
    }
  });

  it("stripping comments does not let a real violation hide behind one", () => {
    // The rules above scan comment-free source. That is only safe if the
    // stripper removes comments and nothing else, so it is checked directly
    // rather than assumed: a declaration must survive, and a token named in a
    // comment must not.
    expect(withoutComments("/* text-faint */\n.a { color: red; }")).toContain("color: red");
    expect(withoutComments("/* text-faint */\n.a { color: red; }")).not.toContain("text-faint");
    expect(withoutComments("// text-faint\n.a { @apply text-ink-faint; }")).toContain("text-ink-faint");
    expect(withoutComments('const u = "https://x/y";')).toContain("https://x/y");
    expect(withoutComments(".a { @apply text-ink-faint; } /* why */")).toContain("text-ink-faint");
  });

  it("glow-3 is reserved for the hero and commit flash", () => {
    const ALLOW = new Set(["src/index.css", "tailwind.config.ts", "src/dev/SpecimenApp.tsx"]);
    for (const f of files) {
      if (ALLOW.has(f.rel) || isLegacy(f.rel)) continue;
      if (f.rel.startsWith("src/hero/")) continue;
      expect(/glow-?3/.test(f.text), `${f.rel} uses glow-3 outside the hero`).toBe(false);
    }
  });

  it("the v2 tokens the rules depend on exist", () => {
    const css = readFileSync(resolve("src/index.css"), "utf8");
    for (const t of ["--live:", "--text-faint:", "--glow-3:", "--brand-text:", "--n0:", "--n9:"]) {
      expect(css).toContain(t);
    }
  });

  it("contrast audit passes (measured ratios, content pairs at 4.5)", () => {
    const out = execFileSync(process.execPath, [resolve("scripts/contrast-audit.mjs")], { encoding: "utf8" });
    expect(out).toContain("self-test: ok (white/black = 21.000");
    expect(out).toContain("gating failures: 0");
  });
});
