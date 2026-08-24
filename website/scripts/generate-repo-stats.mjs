#!/usr/bin/env node
// Repo stats generator: derives the website's code metrics from the Rust tree
// at build time. Pure Node file walking. No network, no git, no shelling out.
//
// Metrics and methods were proven in the Phase 0 feasibility pass:
//   linesOfRust        newline count over crates/**/*.rs (build dirs excluded)
//   tests              #[test] / #[tokio::test] attributes across crates/, sdk/
//   crates             direct subdirectories of crates/ containing a Cargo.toml
//   unsafeBlocks       syntactic unsafe forms only (a comment saying the word
//                      "unsafe" must not count)
//   txTypes            pub const *_PAYLOAD_V1: u8 discriminants in
//                      crates/execution/src/lib.rs, contiguity asserted
//   signalTypes        unit variants of enum AiSignalType, contiguity asserted
//   memoryObjectTypes  unit variants of enum MemoryObjectType, contiguity asserted
//
// Determinism: if every value equals the existing output file's values, the
// previous generatedAt is preserved, so unchanged trees produce byte-identical
// output and both the determinism test and the --check staleness gate are
// plain byte comparisons.
//
// Fail-loud: a metric that cannot be computed, computes to zero when it must
// not, or fails its contiguity assertion exits non-zero and writes nothing.
//
// Usage:
//   node scripts/generate-repo-stats.mjs [--root <repoRoot>] [--out <file>] [--check]
//   --check compares a fresh run against the committed file and fails on drift.

import { readdirSync, readFileSync, statSync, existsSync, writeFileSync, renameSync, mkdirSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));

function parseArgs(argv) {
  const args = { root: resolve(SCRIPT_DIR, "..", ".."), out: resolve(SCRIPT_DIR, "..", "src", "data", "repo-stats.generated.json"), check: false };
  for (let i = 2; i < argv.length; i++) {
    if (argv[i] === "--root") args.root = resolve(argv[++i]);
    else if (argv[i] === "--out") args.out = resolve(argv[++i]);
    else if (argv[i] === "--check") args.check = true;
    else fail(`unknown argument: ${argv[i]}`);
  }
  return args;
}

function fail(msg) {
  console.error(`repo-stats: FAIL: ${msg}`);
  process.exit(1);
}

const SKIP_DIRS = new Set(["target", "node_modules", "dist", "__pycache__"]);

function* walkRs(dir) {
  let entries;
  try {
    entries = readdirSync(dir).sort();
  } catch {
    return;
  }
  for (const name of entries) {
    if (name.startsWith(".")) continue;
    const path = join(dir, name);
    const st = statSync(path);
    if (st.isDirectory()) {
      if (!SKIP_DIRS.has(name)) yield* walkRs(path);
    } else if (st.isFile() && name.endsWith(".rs")) {
      yield path;
    }
  }
}

function countNewlines(buf) {
  let n = 0;
  for (let i = 0; i < buf.length; i++) if (buf[i] === 10) n += 1;
  return n;
}

// Extract the body of `pub enum <name> { ... }` by name-anchored brace matching.
function enumBody(source, name, file) {
  const anchor = source.indexOf(`pub enum ${name}`);
  if (anchor === -1) fail(`enum ${name} not found in ${file}`);
  const open = source.indexOf("{", anchor);
  if (open === -1) fail(`enum ${name}: opening brace not found in ${file}`);
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(open + 1, i);
    }
  }
  fail(`enum ${name}: unbalanced braces in ${file}`);
}

function countUnitVariants(body, name, file) {
  const discriminants = [];
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (t.startsWith("//") || t.startsWith("#")) continue;
    const m = /^([A-Za-z][A-Za-z0-9_]*)\s*=\s*(\d+)\s*,?$/.exec(t);
    if (m) discriminants.push(Number(m[2]));
  }
  if (discriminants.length === 0) fail(`enum ${name}: zero variants matched in ${file}`);
  const sorted = [...discriminants].sort((a, b) => a - b);
  sorted.forEach((d, i) => {
    if (d !== i) fail(`enum ${name}: discriminants not contiguous 0..${sorted.length - 1} (saw ${d} at position ${i}); the counting method needs re-verification`);
  });
  return discriminants.length;
}

function compute(root) {
  const cratesDir = join(root, "crates");
  const sdkDir = join(root, "sdk");
  if (!existsSync(cratesDir)) fail(`crates/ not found under root ${root}`);

  // linesOfRust over crates/ only
  let lines = 0;
  let rsFiles = 0;
  for (const f of walkRs(cratesDir)) {
    lines += countNewlines(readFileSync(f));
    rsFiles += 1;
  }
  if (rsFiles === 0) fail(`zero .rs files under ${cratesDir}`);
  if (lines === 0) fail("linesOfRust computed to zero");

  // tests over crates/ + sdk/
  const testRe = /#\[(?:tokio::)?test(?:\(|\])/g;
  const unsafeRe = /\bunsafe\s*(?:\{|fn\b|impl\b|trait\b|extern\b)/g;
  let tests = 0;
  let unsafeBlocks = 0;
  const testRoots = [cratesDir];
  if (existsSync(sdkDir)) testRoots.push(sdkDir);
  for (const rootDir of testRoots) {
    for (const f of walkRs(rootDir)) {
      const s = readFileSync(f, "utf8");
      tests += (s.match(testRe) ?? []).length;
      if (rootDir === cratesDir) unsafeBlocks += (s.match(unsafeRe) ?? []).length;
    }
  }
  if (tests === 0) fail("tests computed to zero");

  // crates
  const crates = readdirSync(cratesDir).sort().filter((name) => {
    const p = join(cratesDir, name);
    return statSync(p).isDirectory() && existsSync(join(p, "Cargo.toml"));
  });
  if (crates.length === 0) fail("crate count computed to zero");

  // txTypes from the payload consts
  const execLib = join(cratesDir, "execution", "src", "lib.rs");
  if (!existsSync(execLib)) fail(`${execLib} not found`);
  const execSrc = readFileSync(execLib, "utf8");
  const payloads = [...execSrc.matchAll(/^pub const [A-Z_]+_PAYLOAD_V1: u8 = (\d+);$/gm)].map((m) => Number(m[1]));
  if (payloads.length === 0) fail("txTypes: zero PAYLOAD_V1 consts matched");
  const sortedPayloads = [...payloads].sort((a, b) => a - b);
  sortedPayloads.forEach((d, i) => {
    if (d !== i + 1) fail(`txTypes: discriminants not contiguous 1..${sortedPayloads.length} (saw ${d} at position ${i}); the counting method needs re-verification`);
  });

  // signal + memory object enums
  const signalsFile = join(cratesDir, "ai_entities", "src", "signals.rs");
  const memoryFile = join(cratesDir, "ai_entities", "src", "memory.rs");
  if (!existsSync(signalsFile)) fail(`${signalsFile} not found`);
  if (!existsSync(memoryFile)) fail(`${memoryFile} not found`);
  const signalTypes = countUnitVariants(enumBody(readFileSync(signalsFile, "utf8"), "AiSignalType", signalsFile), "AiSignalType", signalsFile);
  const memoryObjectTypes = countUnitVariants(enumBody(readFileSync(memoryFile, "utf8"), "MemoryObjectType", memoryFile), "MemoryObjectType", memoryFile);

  return {
    linesOfRust: { value: lines, method: "newline count over crates/**/*.rs (target and build dirs excluded)" },
    tests: { value: tests, method: "count of #[test] and #[tokio::test] attributes across crates/ and sdk/" },
    crates: { value: crates.length, method: "direct subdirectories of crates/ containing a Cargo.toml" },
    unsafeBlocks: { value: unsafeBlocks, method: "syntactic unsafe forms (unsafe brace/fn/impl/trait/extern) in crates/**/*.rs; prose mentions excluded" },
    txTypes: { value: payloads.length, method: "pub const *_PAYLOAD_V1: u8 discriminants in crates/execution/src/lib.rs, contiguity asserted" },
    signalTypes: { value: signalTypes, method: "unit variants of enum AiSignalType in crates/ai_entities/src/signals.rs, contiguity asserted" },
    memoryObjectTypes: { value: memoryObjectTypes, method: "unit variants of enum MemoryObjectType in crates/ai_entities/src/memory.rs, contiguity asserted" },
  };
}

function render(metrics, generatedAt) {
  return JSON.stringify({ generatedAt, ...metrics }, null, 2) + "\n";
}

function main() {
  const args = parseArgs(process.argv);
  const metrics = compute(args.root);

  let previous = null;
  if (existsSync(args.out)) {
    try {
      previous = JSON.parse(readFileSync(args.out, "utf8"));
    } catch {
      previous = null; // malformed committed file: treat as absent, regenerate fresh
    }
  }

  const valuesUnchanged =
    previous !== null &&
    typeof previous.generatedAt === "string" &&
    Object.entries(metrics).every(
      ([k, v]) => previous[k] && previous[k].value === v.value && previous[k].method === v.method
    ) &&
    Object.keys(previous).length === Object.keys(metrics).length + 1;

  const generatedAt = valuesUnchanged ? previous.generatedAt : new Date().toISOString();
  const output = render(metrics, generatedAt);

  if (args.check) {
    if (!existsSync(args.out)) fail(`--check: ${args.out} does not exist; run npm run stats and commit it`);
    const committed = readFileSync(args.out, "utf8");
    if (committed !== output) fail(`--check: ${args.out} is stale relative to the tree; run npm run stats and commit the result`);
    console.log("repo-stats: check ok (committed file matches a fresh run)");
    return;
  }

  mkdirSync(dirname(args.out), { recursive: true });
  const tmp = args.out + ".tmp";
  writeFileSync(tmp, output);
  renameSync(tmp, args.out);
  console.log(`repo-stats: wrote ${args.out}${valuesUnchanged ? " (values unchanged, timestamp preserved)" : ""}`);
  for (const [k, v] of Object.entries(metrics)) console.log(`  ${k}: ${v.value}`);
}

main();
