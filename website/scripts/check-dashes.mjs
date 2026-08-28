#!/usr/bin/env node
// Dash gate: refuse em/en/figure/bar dashes and the unicode minus anywhere in
// the website tree, including index.html. The ASCII hyphen-minus is the only
// legal dash. Box-drawing characters (U+2500) and middots (U+00B7) are legal.
//
// Exit 0 clean, exit 1 with file:line:col listings otherwise.
// Scans text files under the website root; node_modules, dist, .git, and
// binary files are skipped. public/hero is skipped (operator-supplied media).

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, dirname, extname, relative } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

const SKIP_DIRS = new Set(["node_modules", "dist", ".git", "hero"]);
const BINARY_EXT = new Set([
  ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".ico",
  ".woff", ".woff2", ".ttf", ".otf", ".eot",
  ".mp4", ".webm", ".mp3", ".pdf", ".zip", ".gz",
]);

// Forbidden code points, matching the repo commit-msg hook's set:
// em dash, en dash, figure dash, horizontal bar, minus sign.
const FORBIDDEN = new Map([
  ["—", "em dash (U+2014)"], // dash-gate-definition
  ["–", "en dash (U+2013)"], // dash-gate-definition
  ["‒", "figure dash (U+2012)"], // dash-gate-definition
  ["―", "horizontal bar (U+2015)"], // dash-gate-definition
  ["−", "minus sign (U+2212)"], // dash-gate-definition
]);
const CHARS = [...FORBIDDEN.keys()].join("");
// Built fresh at each use. A shared /g/ regex carries lastIndex between calls,
// and String.matchAll inherits that lastIndex, which is how this gate used to
// scan every line from an offset past the violation and report nothing.
const pattern = () => new RegExp(`[${CHARS}]`, "g");
const contains = (s) => new RegExp(`[${CHARS}]`).test(s);

// A file that DEFINES the forbidden set has to contain the set. The exemption
// is deliberately two conditions, path and marker, so it cannot be used to
// silence an ordinary violation: an allowlisted file is still fully scanned on
// every line that does not carry the marker, and the marker means nothing in
// any other file.
const MARKER = "dash-gate-definition";
const DEFINITION_SITES = new Set([
  "scripts/check-dashes.mjs",
  "scripts/generate-console-data.mjs",
]);

function* walk(dir) {
  for (const name of readdirSync(dir).sort()) {
    if (name.startsWith(".")) continue;
    const path = join(dir, name);
    const st = statSync(path);
    if (st.isDirectory()) {
      if (!SKIP_DIRS.has(name)) yield* walk(path);
    } else if (st.isFile()) {
      yield path;
    }
  }
}

let violations = 0;
let exempted = 0;
const usedDefinitionSites = new Set();

for (const path of walk(ROOT)) {
  if (BINARY_EXT.has(extname(path).toLowerCase())) continue;
  const buf = readFileSync(path);
  if (buf.subarray(0, 8192).includes(0)) continue; // binary content
  const text = buf.toString("utf8");
  if (!contains(text)) continue;
  const rel = relative(ROOT, path);
  const isDefinitionSite = DEFINITION_SITES.has(rel);
  const lines = text.split("\n");
  lines.forEach((line, i) => {
    for (const match of line.matchAll(pattern())) {
      if (isDefinitionSite && line.includes(MARKER)) {
        exempted += 1;
        usedDefinitionSites.add(rel);
        continue;
      }
      violations += 1;
      const label = FORBIDDEN.get(match[0]);
      console.error(
        `${relative(ROOT, path)}:${i + 1}:${match.index + 1} ${label}`
      );
      console.error(`    ${line.trim()}`);
    }
  });
}

// The allowlist can only shrink. A path that no longer needs its exemption
// fails the gate naming itself, so the list cannot quietly become furniture.
for (const site of [...DEFINITION_SITES].sort()) {
  if (!usedDefinitionSites.has(site)) {
    console.error(
      `dash gate: ${site} is listed as a definition site but no longer carries a ${MARKER} line. Remove it from DEFINITION_SITES.`
    );
    violations += 1;
  }
}

if (violations > 0) {
  console.error(`\ndash gate: ${violations} violation(s). Use the ASCII hyphen.`);
  process.exit(1);
}
console.log(`dash gate: clean (${exempted} definition-site occurrence(s) exempted across ${usedDefinitionSites.size} file(s))`);
