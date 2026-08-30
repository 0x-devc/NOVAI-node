#!/usr/bin/env node
//
// Every built page's classes actually survived into a stylesheet it loads.
//
// THE FAILURE THIS EXISTS FOR, which has already happened once here:
// tailwind.config.ts lists content globs, Tailwind reads those files from disk,
// and any page not matched by a glob has EVERY utility class on it purged. Dev
// does not purge, so the page looks correct locally and ships unstyled. The
// console went multi-page in this gate, from one HTML entry to ten, and each
// new file is a fresh chance to miss one.
//
// Two checks, and only the second is worth much:
//
//   1. Config coverage. Every HTML entry in vite.config.ts's rollupOptions is
//      matched by at least one Tailwind content glob. This reads intent.
//   2. Output coverage. For every built page, a class that appears in THAT
//      page's markup has a rule in a stylesheet THAT page links. This reads the
//      artifact, and it is the one that would have caught the original bug.
//
// Check 2 is the automated form of the manual "does list-disc appear in the
// built CSS" check that found this the first time.
//
// Usage:
//   node scripts/check-css-coverage.mjs          config only
//   node scripts/check-css-coverage.mjs --dist   config and built output
//

import { readFileSync, existsSync, readdirSync } from "node:fs";
import { dirname, join, resolve, relative } from "node:path";
import { fileURLToPath } from "node:url";

const WEB_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIST = join(WEB_ROOT, "dist");

function fail(msg) {
  console.error(`css-coverage: FAIL: ${msg}`);
  process.exit(1);
}

/** The HTML entries vite is told to build. */
function viteEntries() {
  const src = readFileSync(join(WEB_ROOT, "vite.config.ts"), "utf8");
  const block = /input:\s*\{([\s\S]*?)\}/.exec(src);
  if (!block) fail("vite.config.ts: no rollupOptions.input object found, so nothing was checked");
  const entries = [...block[1].matchAll(/path\.resolve\(__dirname,\s*"([^"]+)"\)/g)].map((m) => m[1]);
  if (entries.length === 0) fail("vite.config.ts: input parsed to zero entries, so the scan is broken");
  return entries;
}

/** The content globs tailwind is told to scan, as anchored regexes. */
function tailwindGlobs() {
  const src = readFileSync(join(WEB_ROOT, "tailwind.config.ts"), "utf8");
  const block = /content:\s*\[([\s\S]*?)\]/.exec(src);
  if (!block) fail("tailwind.config.ts: no content array found, so nothing was checked");
  const globs = [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  if (globs.length === 0) fail("tailwind.config.ts: content parsed to zero globs, so the scan is broken");
  return globs.map((g) => ({
    glob: g,
    re: new RegExp(
      "^" +
        g
          .replace(/^\.\//, "")
          .replace(/[.+^${}()|[\]\\]/g, "\\$&")
          .replace(/\*\*\//g, "(?:.*/)?")
          .replace(/\*/g, "[^/]*")
          .replace(/\{([^}]*)\}/g, (_, alts) => `(?:${alts.split(",").join("|")})`) +
        "$"
    ),
  }));
}

function checkConfig() {
  const globs = tailwindGlobs();
  // Only pages whose OWN markup carries classes need a glob. index.html is a
  // React shell with a root div and no class attribute at all, so its classes
  // come from .tsx files that are already scanned; requiring a glob for it
  // would be a rule about nothing. The console pages are the opposite case:
  // their markup IS the content.
  const carriesClasses = (rel) => {
    const path = join(WEB_ROOT, rel);
    if (!existsSync(path)) fail(`vite.config.ts names ${rel}, which does not exist`);
    return /class="/.test(readFileSync(path, "utf8"));
  };
  const html = viteEntries().filter((e) => e.endsWith(".html"));
  const needGlob = html.filter(carriesClasses);
  if (needGlob.length === 0) fail("no HTML entry carries classes in its own markup, so this check saw nothing");
  const missed = needGlob.filter((e) => !globs.some((g) => g.re.test(e)));
  if (missed.length) {
    fail(
      `these built pages are matched by no Tailwind content glob, so every utility class on them is purged ` +
        `and they ship unstyled in production while looking correct in dev: ${missed.join(", ")}`
    );
  }
  console.log(
    `css-coverage: config ok (${needGlob.length} of ${html.length} HTML entries carry their own classes, all matched by ${globs.length} globs)`
  );
}

function checkDist() {
  if (!existsSync(DIST)) fail("dist/ does not exist; run the build before checking its output");
  const pages = [];
  const walk = (dir) => {
    for (const name of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, name.name);
      if (name.isDirectory()) walk(full);
      else if (name.name.endsWith(".html")) pages.push(full);
    }
  };
  walk(DIST);
  if (pages.length === 0) fail("dist/ contains no HTML pages, so nothing was checked");

  let probes = 0;
  let skipped = 0;
  for (const page of pages) {
    const html = readFileSync(page, "utf8");
    const rel = relative(DIST, page);
    const sheets = [...html.matchAll(/href="([^"]+\.css)"/g)].map((m) => m[1].replace(/^\//, ""));
    if (sheets.length === 0) fail(`${rel} links no stylesheet, so nothing it renders can be styled`);
    const css = sheets.map((s) => readFileSync(join(DIST, s), "utf8")).join("\n");

    // Probe with classes that exist in this page's markup and nowhere in the
    // TypeScript sources, so a rule for them can only come from scanning this
    // file. A class shared with a React component would pass by accident.
    const classes = new Set([...html.matchAll(/class="([^"]*)"/g)].flatMap((m) => m[1].split(/\s+/)));
    const candidates = [...classes].filter((c) => /^(console-|tok-|list-disc$|pl-5$)/.test(c));
    if (candidates.length === 0) {
      // A page whose source carries no classes is a React shell, and its
      // classes come from .tsx files that are scanned anyway. A page whose
      // source DID carry classes and whose output carries none rendered empty,
      // which is a real failure.
      const src = join(WEB_ROOT, rel);
      if (existsSync(src) && /class="/.test(readFileSync(src, "utf8"))) {
        fail(`${rel} carries classes in source and none in the built output, so it rendered empty`);
      }
      skipped += 1;
      continue;
    }
    for (const c of candidates) {
      const escaped = c.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
      if (!new RegExp(`\\.${escaped}[\\s,{:>~+\\[]`).test(css)) {
        fail(
          `${rel} uses .${c} and no stylesheet it links defines it. Tailwind purged it, which means this page ` +
            `is missing from the content globs in tailwind.config.ts.`
        );
      }
      probes += 1;
    }
  }
  console.log(
    `css-coverage: output ok (${pages.length} built pages, ${skipped} shell page(s) skipped, ${probes} class probes all resolved)`
  );
}

checkConfig();
if (process.argv.includes("--dist")) checkDist();
