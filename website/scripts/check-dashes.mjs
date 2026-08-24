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
  ["—", "em dash (U+2014)"],
  ["–", "en dash (U+2013)"],
  ["‒", "figure dash (U+2012)"],
  ["―", "horizontal bar (U+2015)"],
  ["−", "minus sign (U+2212)"],
]);
const PATTERN = new RegExp(`[${[...FORBIDDEN.keys()].join("")}]`, "g");

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
for (const path of walk(ROOT)) {
  if (BINARY_EXT.has(extname(path).toLowerCase())) continue;
  const buf = readFileSync(path);
  if (buf.subarray(0, 8192).includes(0)) continue; // binary content
  const text = buf.toString("utf8");
  if (!PATTERN.test(text)) continue;
  const lines = text.split("\n");
  lines.forEach((line, i) => {
    for (const match of line.matchAll(PATTERN)) {
      violations += 1;
      const label = FORBIDDEN.get(match[0]);
      console.error(
        `${relative(ROOT, path)}:${i + 1}:${match.index + 1} ${label}`
      );
      console.error(`    ${line.trim()}`);
    }
  });
}

if (violations > 0) {
  console.error(`\ndash gate: ${violations} violation(s). Use the ASCII hyphen.`);
  process.exit(1);
}
console.log("dash gate: clean");
