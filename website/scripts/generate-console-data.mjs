#!/usr/bin/env node
// Console data generator: derives the developer console's reference material
// from the repository. Pure Node file reading. No network, no git, no shelling
// out. Reads only from the repo root; writes only inside website/.
//
// Outputs, both from one parse so they cannot disagree:
//   src/data/console-data.generated.json   the page's data
//   public/openrpc.json                    the machine-readable service description
//
// The doc is parsed against its REAL grammar, which is irregular. Measured on
// 2026-08-28 across the 29 methods:
//   29 have a curl example      25 have a Params block      18 have a Result fence
//   21 carry error codes of their own                       19 have a sample response
// The shortfall is not missing information. It is inheritance and aliasing:
//   - three Result blocks say "same shape as <method>" and one says "identical
//     to <method>"; three Params blocks and five Errors blocks alias likewise
//   - seventeen methods inherit a record shape declared once in their category
//     preamble: three of them declare no result at all, and fourteen declare
//     only an envelope such as { "agreements": [SlaAgreement, ...] } whose
//     record type is defined in the preamble. The three signal methods also
//     inherit a Common errors table.
// A parser that ignores either mechanism renders about a third of the surface
// silently wrong while looking complete, so both are mandatory here.
//
// Three grammar traps, each defused structurally rather than by heuristic:
//   1. novai_faucet puts a Cooldowns table BEFORE its Params table. Blocks are
//      delimited by their bold label, never by table order, so "the first
//      table" is never consulted.
//   2. Three error tables repeat a code key (novai_faucet lists -32000 twice).
//      Errors are therefore ALWAYS a list of {code, when} and never a map.
//   3. One label, "**Building the hex.**", carries its period inside the bold
//      and is followed by prose rather than a colon. The label match is
//      non-greedy and a label whose remainder is not a colon or a
//      parenthetical is recorded as prose, so it terminates the block above it
//      instead of being swallowed into it.
//
// Drift gate: four-way name-set equality across independent sources
//   1. dispatch arms in crates/node/src/rpc.rs
//   2. "### " method headings in docs/RPC_REFERENCE.md
//   3. the method table in README.md
//   4. the methods sdk/novai-python-sdk actually calls
// The fourth is the strongest signal because it is executable code, not prose.
// rpc.rs contains doc-comment references to novai_getStatus, a method that does
// not exist, so comments are stripped before any scan.
//
// KNOWN_DRIFT fails in BOTH directions: on new drift, and on a listed
// exception that has stopped applying, naming the entry to delete. Without the
// second direction the list is furniture that only ever grows.
//
// Usage:
//   node scripts/generate-console-data.mjs [--root <repoRoot>] [--out <file>]
//                                          [--openrpc <file>] [--check]
//   --check compares a fresh run against both committed files and fails on drift.

import { readdirSync, readFileSync, statSync, existsSync, writeFileSync, renameSync, mkdirSync } from "node:fs";
import { join, dirname, resolve, relative } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));

function fail(msg) {
  console.error(`console-data: FAIL: ${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = {
    root: resolve(SCRIPT_DIR, "..", ".."),
    out: resolve(SCRIPT_DIR, "..", "src", "data", "console-data.generated.json"),
    openrpc: resolve(SCRIPT_DIR, "..", "public", "openrpc.json"),
    check: false,
  };
  for (let i = 2; i < argv.length; i++) {
    if (argv[i] === "--root") args.root = resolve(argv[++i]);
    else if (argv[i] === "--out") args.out = resolve(argv[++i]);
    else if (argv[i] === "--openrpc") args.openrpc = resolve(argv[++i]);
    else if (argv[i] === "--check") args.check = true;
    else fail(`unknown argument: ${argv[i]}`);
  }
  return args;
}

// ---------------------------------------------------------------------------
// Character normalisation
//
// check-dashes.mjs forbids five code points anywhere under website/. The doc
// contains one of them (a unicode minus in the signal range-error row), so
// copying doc text verbatim would fail the build. Substitution is explicit and
// every replacement is logged with its source location, so it is proven rather
// than assumed. The generated file is run through the dash gate by the suite.
// ---------------------------------------------------------------------------

const SUBSTITUTIONS = new Map([
  ["—", { to: "-", name: "em dash (U+2014)" }], // dash-gate-definition
  ["–", { to: "-", name: "en dash (U+2013)" }], // dash-gate-definition
  ["‒", { to: "-", name: "figure dash (U+2012)" }], // dash-gate-definition
  ["―", { to: "-", name: "horizontal bar (U+2015)" }], // dash-gate-definition
  ["−", { to: "-", name: "minus sign (U+2212)" }], // dash-gate-definition
]);
const SUB_CHARS = [...SUBSTITUTIONS.keys()].join("");
// Built fresh at each use. A shared /g/ regex carries lastIndex between calls
// and String.matchAll inherits it, which silently skips matches. The dash gate
// had exactly that defect.
const subPattern = () => new RegExp(`[${SUB_CHARS}]`, "g");
const hasForbidden = (s) => new RegExp(`[${SUB_CHARS}]`).test(s);

const substitutionLog = [];

function normaliseDashes(text, label) {
  if (!hasForbidden(text)) return text;
  text.split("\n").forEach((line, i) => {
    for (const m of line.matchAll(subPattern())) {
      substitutionLog.push({
        source: label,
        line: i + 1,
        column: m.index + 1,
        from: SUBSTITUTIONS.get(m[0]).name,
        to: SUBSTITUTIONS.get(m[0]).to,
      });
    }
  });
  return text.replace(subPattern(), (c) => SUBSTITUTIONS.get(c).to);
}

// Memoised, so a file read twice is normalised and logged once. Without this
// the substitution log would count reads rather than occurrences, which is
// exactly the kind of instrument error that makes a measurement worthless.
const textCache = new Map();

function readText(root, rel) {
  const path = join(root, rel);
  if (textCache.has(path)) return textCache.get(path);
  if (!existsSync(path)) fail(`${rel} not found under root ${root}`);
  const text = normaliseDashes(readFileSync(path, "utf8"), rel);
  textCache.set(path, text);
  return text;
}

/** 1-based line number of a character offset. */
function lineOf(text, index) {
  let n = 1;
  for (let i = 0; i < index && i < text.length; i++) if (text[i] === "\n") n += 1;
  return n;
}

// Integer expression evaluator for constant right-hand sides such as
// `512 * 1024`, `32 + 8 + 32` or `2 * ((n - 1) / 3) + 1`.
//
// Deliberately not eval and not new Function: the input is Rust source read
// off disk, and a parser that only knows integers and four operators cannot be
// talked into running anything. Division floors, matching Rust integer
// division, because the quorum formula depends on it.
function evalIntExpr(expr, where, vars = {}) {
  let src = expr.replace(/_/g, "");
  for (const [name, value] of Object.entries(vars)) {
    src = src.replace(new RegExp(`\\b${name}\\b`, "g"), String(value));
  }
  if (!/^[0-9\s()*+/-]+$/.test(src)) {
    fail(`${where}: not a plain integer expression: ${expr}`);
  }
  let i = 0;
  function ws() { while (i < src.length && /\s/.test(src[i])) i += 1; }
  function primary() {
    ws();
    if (src[i] === "(") {
      i += 1;
      const v = sum();
      ws();
      if (src[i] !== ")") fail(`${where}: unbalanced parentheses in ${expr}`);
      i += 1;
      return v;
    }
    const m = /^\d+/.exec(src.slice(i));
    if (!m) fail(`${where}: expected a number at offset ${i} in ${expr}`);
    i += m[0].length;
    return Number(m[0]);
  }
  function product() {
    let v = primary();
    for (;;) {
      ws();
      const op = src[i];
      if (op !== "*" && op !== "/") return v;
      i += 1;
      const r = primary();
      if (op === "/" && r === 0) fail(`${where}: division by zero in ${expr}`);
      v = op === "*" ? v * r : Math.floor(v / r);
    }
  }
  function sum() {
    let v = product();
    for (;;) {
      ws();
      const op = src[i];
      if (op !== "+" && op !== "-") return v;
      i += 1;
      const r = product();
      v = op === "+" ? v + r : v - r;
    }
  }
  const out = sum();
  ws();
  if (i !== src.length) fail(`${where}: trailing characters in ${expr}`);
  if (!Number.isSafeInteger(out)) fail(`${where}: ${expr} is not a safe integer`);
  return out;
}

// ---------------------------------------------------------------------------
// Rust source scanning
// ---------------------------------------------------------------------------

// Remove comments while leaving string literals intact, and return a parallel
// "masked" copy in which string contents are blanked too, so that brace depth
// can be counted without a brace inside a literal throwing it off.
//
// Handles line comments, nesting block comments, string literals with escapes,
// raw strings of any hash depth, and char literals. Char literals matter
// because a lifetime (&'a T) looks like an unterminated one; the disambiguation
// is that a char literal closes within three characters.
//
// Every branch preserves length, and that is asserted, so reported line and
// column numbers stay true to the source.
function scanRust(src, label) {
  let code = "";
  let masked = "";
  let i = 0;
  const n = src.length;
  while (i < n) {
    const c = src[i];
    // raw string: r"..." or r#"..."#
    if (c === "r" && (src[i + 1] === '"' || src[i + 1] === "#")) {
      let hashes = 0;
      let j = i + 1;
      while (src[j] === "#") { hashes += 1; j += 1; }
      if (src[j] === '"') {
        const terminator = '"' + "#".repeat(hashes);
        const end = src.indexOf(terminator, j + 1);
        if (end === -1) fail(`${label}: unterminated raw string literal`);
        const literal = src.slice(i, end + terminator.length);
        code += literal;
        masked += literal.replace(/[^\n]/g, " ");
        i = end + terminator.length;
        continue;
      }
    }
    if (c === '"') {
      let j = i + 1;
      while (j < n) {
        if (src[j] === "\\") { j += 2; continue; }
        if (src[j] === '"') break;
        j += 1;
      }
      if (j >= n) fail(`${label}: unterminated string literal`);
      const literal = src.slice(i, j + 1);
      code += literal;
      masked += literal.replace(/[^\n]/g, " ");
      i = j + 1;
      continue;
    }
    if (c === "/" && src[i + 1] === "/") {
      let j = i;
      while (j < n && src[j] !== "\n") j += 1;
      const run = src.slice(i, j);
      code += run.replace(/[^\n]/g, " ");
      masked += run.replace(/[^\n]/g, " ");
      i = j;
      continue;
    }
    if (c === "/" && src[i + 1] === "*") {
      let depth = 0;
      let j = i;
      while (j < n) {
        if (src[j] === "/" && src[j + 1] === "*") { depth += 1; j += 2; continue; }
        if (src[j] === "*" && src[j + 1] === "/") { depth -= 1; j += 2; if (depth === 0) break; continue; }
        j += 1;
      }
      if (depth !== 0) fail(`${label}: unterminated block comment`);
      const run = src.slice(i, j);
      code += run.replace(/[^\n]/g, " ");
      masked += run.replace(/[^\n]/g, " ");
      i = j;
      continue;
    }
    // Char literal, not a lifetime: it closes within three characters.
    if (c === "'") {
      let end = -1;
      if (src[i + 1] === "\\") {
        for (let j = i + 2; j < n && j < i + 8; j++) if (src[j] === "'") { end = j; break; }
      } else if (src[i + 2] === "'") {
        end = i + 2;
      }
      if (end !== -1) {
        const literal = src.slice(i, end + 1);
        code += literal;
        masked += literal.replace(/[^\n]/g, " ");
        i = end + 1;
        continue;
      }
    }
    code += c;
    masked += c;
    i += 1;
  }
  if (code.length !== src.length || masked.length !== src.length) {
    fail(`${label}: scanner did not preserve length; line numbers would be wrong`);
  }
  return { code, masked };
}

function* walkFiles(dir, ext) {
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
      if (name === "target" || name === "node_modules" || name === "__pycache__") continue;
      yield* walkFiles(path, ext);
    } else if (st.isFile() && name.endsWith(ext)) {
      yield path;
    }
  }
}

const ARM_LINE = /^\s*"([^"]*)"\s*=>/;
const DISPATCH_SHAPED = /^[ \t]*"novai_[A-Za-z]+"[ \t]*=>/m;

// Source 1: the dispatch arms. Located by brace-slicing the match block rather
// than by an indentation anchor, so a rustfmt pass cannot disarm the scan.
function dispatchMethods(root) {
  const rel = "crates/node/src/rpc.rs";
  const raw = readFileSync(join(root, rel), "utf8");
  const { code, masked } = scanRust(raw, rel);

  const anchor = /match\s+[A-Za-z_][A-Za-z0-9_.]*\.method\.as_str\(\)\s*\{/.exec(code);
  if (!anchor) fail(`${rel}: method dispatch match block not found`);
  const open = anchor.index + anchor[0].length - 1;

  let depth = 0;
  let close = -1;
  for (let i = open; i < masked.length; i++) {
    if (masked[i] === "{") depth += 1;
    else if (masked[i] === "}") {
      depth -= 1;
      if (depth === 0) { close = i; break; }
    }
  }
  if (close === -1) fail(`${rel}: unbalanced braces in the dispatch match block`);

  const bodyCode = code.slice(open + 1, close);
  const bodyMasked = masked.slice(open + 1, close);

  // Collect arms that sit at depth 0 relative to the match body.
  //
  // armLine records the ABSOLUTE 1-based line of each arm in rpc.rs, which is
  // what the console's per-method source links point at. scanRust preserves
  // length, so a line number in `code` is a line number in the real file.
  // The link is only as good as this number, so compute(root) re-asserts that
  // the method name still appears at its recorded line and fails when it
  // moves: a link that still resolves but points at the wrong line rots
  // silently, which is worse than one that 404s.
  const bodyFirstLine = lineOf(code, open + 1);
  const armLine = new Map();
  const arms = [];
  let d = 0;
  const lines = bodyCode.split("\n");
  const maskedLines = bodyMasked.split("\n");
  for (let li = 0; li < lines.length; li++) {
    const depthAtLineStart = d;
    for (const ch of maskedLines[li]) {
      if (ch === "{" || ch === "(" || ch === "[") d += 1;
      else if (ch === "}" || ch === ")" || ch === "]") d -= 1;
    }
    if (depthAtLineStart !== 0) continue;
    const line = lines[li];
    if (/^\s*$/.test(line)) continue;
    const m = ARM_LINE.exec(line);
    if (m) {
      if (/\bif\b/.test(line.slice(0, line.indexOf("=>")))) {
        fail(`${rel}: dispatch arm has a match guard, which the scan cannot resolve: ${line.trim()}`);
      }
      if (/"\s*\|/.test(line.slice(0, line.indexOf("=>")))) {
        fail(`${rel}: dispatch arm uses an or-pattern, which would hide an alias: ${line.trim()}`);
      }
      arms.push(m[1]);
      armLine.set(m[1], bodyFirstLine + li);
      continue;
    }
    if (/^\s*_\s*=>/.test(line)) {
      arms.push(null); // catch-all
      continue;
    }
    if (/=>/.test(line)) {
      fail(`${rel}: dispatch arm is not a bare string literal or the catch-all: ${line.trim()}`);
    }
  }

  const catchAlls = arms.filter((a) => a === null).length;
  if (catchAlls !== 1) fail(`${rel}: expected exactly one catch-all dispatch arm, found ${catchAlls}`);
  const names = arms.filter((a) => a !== null);
  const prefixed = names.filter((a) => a.startsWith("novai_"));
  if (prefixed.length !== names.length) {
    const other = names.filter((a) => !a.startsWith("novai_"));
    fail(`${rel}: dispatch arm(s) outside the novai_ prefix would be missed by the drift gate: ${other.join(", ")}`);
  }
  if (names.length === 0) fail(`${rel}: zero dispatch arms matched`);

  // No second dispatch site anywhere under crates/.
  const others = [];
  for (const f of walkFiles(join(root, "crates"), ".rs")) {
    const rf = relative(root, f);
    if (rf === rel) continue;
    const { code: c } = scanRust(readFileSync(f, "utf8"), rf);
    if (DISPATCH_SHAPED.test(c)) others.push(rf);
  }
  if (others.length > 0) {
    fail(`dispatch-shaped lines found outside ${rel}, so the drift gate reads only part of the surface: ${others.join(", ")}`);
  }

  return { names: new Set(names), source: rel, emitted: code, armLine };
}

// ---------------------------------------------------------------------------
// Markdown helpers
// ---------------------------------------------------------------------------

// Mark which lines sit inside a fenced code block, so that no heading, label
// or table inside a fence is ever read as document structure.
function fenceMap(lines) {
  const inFence = new Array(lines.length).fill(false);
  let open = null;
  for (let i = 0; i < lines.length; i++) {
    const m = /^\s*(`{3,})(.*)$/.exec(lines[i]);
    if (m) {
      if (open === null) {
        open = m[1];
        inFence[i] = true;
        continue;
      }
      if (m[1].length >= open.length && m[2].trim() === "") {
        inFence[i] = true;
        open = null;
        continue;
      }
    }
    inFence[i] = open !== null;
  }
  if (open !== null) fail("unterminated code fence in docs/RPC_REFERENCE.md");
  return inFence;
}

// A markdown table starting at `start`. Header-driven, so a two-column table
// and a three-column table both parse. Returns null when there is no table.
function parseTable(lines, inFence, start, end) {
  let i = start;
  while (i < end && (lines[i].trim() === "" || !lines[i].trim().startsWith("|"))) {
    if (lines[i].trim() !== "" && !lines[i].trim().startsWith("|")) return null;
    i += 1;
  }
  if (i + 1 >= end) return null;
  if (inFence[i]) return null;
  const header = splitRow(lines[i]);
  if (!/^\|[\s:|-]+\|$/.test(lines[i + 1].trim())) return null;
  const rows = [];
  let j = i + 2;
  for (; j < end; j++) {
    const t = lines[j].trim();
    if (!t.startsWith("|")) break;
    const cells = splitRow(lines[j]);
    const row = {};
    header.forEach((h, k) => { row[h] = cells[k] ?? ""; });
    rows.push(row);
  }
  if (rows.length === 0) return null;
  return { header, rows };
}

function splitRow(line) {
  const t = line.trim().replace(/^\|/, "").replace(/\|$/, "");
  return t.split("|").map((c) => c.trim());
}

function firstFence(lines, inFence, start, end) {
  for (let i = start; i < end; i++) {
    const m = /^\s*```(\w*)\s*$/.exec(lines[i]);
    if (!m) continue;
    for (let j = i + 1; j < end; j++) {
      if (/^\s*```\s*$/.test(lines[j])) {
        return { lang: m[1] || "text", body: lines.slice(i + 1, j).join("\n"), start: i, end: j };
      }
    }
    fail(`unterminated fence at docs/RPC_REFERENCE.md line ${i + 1}`);
  }
  return null;
}

function fencesOfLang(lines, start, end, lang) {
  const out = [];
  for (let i = start; i < end; i++) {
    const m = /^\s*```(\w*)\s*$/.exec(lines[i]);
    if (!m || (m[1] || "text") !== lang) continue;
    for (let j = i + 1; j < end; j++) {
      if (/^\s*```\s*$/.test(lines[j])) {
        out.push({ body: lines.slice(i + 1, j).join("\n"), start: i, end: j });
        i = j;
        break;
      }
    }
  }
  return out;
}

const stripTicks = (s) => s.replace(/`/g, "").trim();

// ---------------------------------------------------------------------------
// The document parse
// ---------------------------------------------------------------------------

// Non-greedy label match. "**Building the hex.**" keeps its period inside the
// bold and is followed by prose, so it is captured as a prose label: unknown to
// the reader below, but still a block boundary, which is what stops it being
// absorbed into the block above.
const LABEL_LINE = /^\*\*(.+?)\*\*(.*)$/;

function readLabel(line) {
  const m = LABEL_LINE.exec(line);
  if (!m) return null;
  const label = m[1].replace(/\.$/, "").trim();
  let rest = m[2];
  let qualifier = null;
  if (rest.startsWith(" (")) {
    let depth = 0;
    let k = 1;
    for (; k < rest.length; k++) {
      if (rest[k] === "(") depth += 1;
      else if (rest[k] === ")") { depth -= 1; if (depth === 0) break; }
    }
    if (depth === 0) {
      qualifier = rest.slice(2, k);
      rest = rest.slice(k + 1);
    }
  }
  let inline = null;
  let prose = false;
  if (rest.startsWith(":")) inline = rest.slice(1).trim();
  else if (rest.trim() === "") inline = "";
  else prose = true;
  return { label, qualifier, inline, prose };
}

function parseDoc(text) {
  const lines = text.split("\n");
  const inFence = fenceMap(lines);

  const categories = [];
  const methods = [];
  for (let i = 0; i < lines.length; i++) {
    if (inFence[i]) continue;
    const cat = /^##\s+(.+?)\s*$/.exec(lines[i]);
    if (cat && !lines[i].startsWith("###")) {
      categories.push({ title: cat[1], start: i, methods: [] });
      continue;
    }
    const meth = /^###\s+`(novai_[A-Za-z]+)`\s*$/.exec(lines[i]);
    if (meth) {
      if (categories.length === 0) fail(`method ${meth[1]} appears before any category heading`);
      methods.push({ name: meth[1], start: i, category: categories[categories.length - 1].title });
      categories[categories.length - 1].methods.push(meth[1]);
      continue;
    }
    if (/^###\s/.test(lines[i])) {
      const other = /^###\s+(.+?)\s*$/.exec(lines[i]);
      // Sub-headings outside the method sections (Field reference) are allowed.
      if (/`novai_/.test(other[1])) fail(`method heading not in the expected form: ${lines[i]}`);
    }
  }
  if (methods.length === 0) fail("zero method headings matched in docs/RPC_REFERENCE.md");

  // Section boundaries.
  const boundaries = [...categories.map((c) => c.start), ...methods.map((m) => m.start)].sort((a, b) => a - b);
  const nextBoundary = (from) => boundaries.find((b) => b > from) ?? lines.length;

  for (const c of categories) c.end = nextBoundary(c.start);
  for (const m of methods) m.end = nextBoundary(m.start);

  return { lines, inFence, categories, methods };
}

// Split a section into label-delimited blocks. A block runs from its label line
// to the next label line or the end of the section, so table order inside the
// section is never consulted (trap 1).
function blocksOf(lines, inFence, start, end) {
  const marks = [];
  for (let i = start; i < end; i++) {
    if (inFence[i]) continue;
    const l = readLabel(lines[i]);
    if (l) marks.push({ ...l, line: i });
  }
  return marks.map((mk, idx) => ({
    ...mk,
    start: mk.line,
    end: idx + 1 < marks.length ? marks[idx + 1].line : end,
  }));
}

const findBlock = (blocks, label) => blocks.find((b) => b.label.toLowerCase() === label.toLowerCase() && !b.prose) ?? null;

// Aliases in the doc drop the novai_ prefix: "same shape as `getLatestBlock`".
function resolveAlias(text, known, context) {
  const m = /(?:same(?:\s+shape)?\s+as|identical\s+to)\s+`([A-Za-z_]+)`/i.exec(text);
  if (!m) return null;
  const bare = m[1];
  const full = bare.startsWith("novai_") ? bare : `novai_${bare}`;
  if (!known.has(full)) fail(`${context}: alias points at "${bare}", which is not a known method`);
  return full;
}

// A factory, not a shared object. Reusing one /g/ regex across calls is how the
// dash gate silently stopped working: matchAll inherits lastIndex from the
// regex it is handed. This site is safe today because nothing advances it, but
// safe-by-accident is one edit away from the same bug.
const errorCodePattern = () => /`(-3\d{4})`/g;

// Errors are ALWAYS a list. Three tables in the doc repeat a code key
// (novai_faucet lists -32000 twice), so a map would silently drop rows (trap 2).
function parseErrors(block, lines, inFence, known, name) {
  if (!block) return null;
  const inline = (block.inline ?? "").trim();

  const alias = inline ? resolveAlias(inline, known, `${name} errors`) : null;
  if (alias) return { kind: "alias", alias };

  const table = parseTable(lines, inFence, block.start + 1, block.end);
  if (table) {
    const codeKey = table.header.find((h) => /code/i.test(h));
    const whenKey = table.header.find((h) => /when|trigger|meaning/i.test(h));
    if (!codeKey || !whenKey) fail(`${name}: errors table header not understood: ${table.header.join(" | ")}`);
    const list = table.rows.map((r) => ({ code: Number(stripTicks(r[codeKey])), when: r[whenKey] }));
    for (const e of list) if (!Number.isInteger(e.code)) fail(`${name}: unparseable error code in the errors table`);
    return { kind: "table", list };
  }

  if (inline) {
    const codes = [...inline.matchAll(errorCodePattern())].map((m) => Number(m[1]));
    if (codes.length === 0) fail(`${name}: errors block has neither a table, an alias, nor any code`);
    // "only the global ones (...)" carries no per-code clause; the prose forms
    // carry one clause per code separated by semicolons.
    const clauses = inline.split(";").map((s) => s.trim()).filter(Boolean);
    const list = codes.map((code) => {
      const clause = clauses.find((c) => c.includes(String(code)));
      return { code, when: clause ? clause.replace(/`/g, "").replace(new RegExp(`^\\s*${code}\\s*(for)?\\s*`), "").replace(/\.$/, "").trim() : null };
    });
    return { kind: "prose", list, text: inline };
  }
  fail(`${name}: errors block is empty`);
}

function parseParams(block, lines, inFence, known, name) {
  if (!block) return null;
  const inline = (block.inline ?? "").trim();

  if (/^none\b/i.test(inline)) return { kind: "none", note: inline };

  const alias = inline ? resolveAlias(inline, known, `${name} params`) : null;
  if (alias) return { kind: "alias", alias, note: inline };

  const table = parseTable(lines, inFence, block.start + 1, block.end);
  if (!table) fail(`${name}: params block has neither a table, an alias, nor "none"`);
  const fieldKey = table.header.find((h) => /field|param/i.test(h));
  const typeKey = table.header.find((h) => /type/i.test(h));
  const notesKey = table.header.find((h) => /note/i.test(h));
  if (!fieldKey || !typeKey) fail(`${name}: params table header not understood: ${table.header.join(" | ")}`);
  const list = table.rows.map((r) => {
    const rawType = r[typeKey];
    const optional = /\(optional\)/i.test(rawType);
    return {
      field: stripTicks(r[fieldKey]),
      type: stripTicks(rawType.replace(/\(optional\)/i, "")),
      optional,
      notes: notesKey ? r[notesKey] : null,
    };
  });
  return { kind: "table", list };
}

// Record-type references in an envelope: a bare CamelCase identifier that is
// not inside a string and not inside a jsonc comment. Full field-by-field
// shapes never contain one (their values are quoted placeholders such as
// "<hex32>", or <u64>, or lowercase literals), while envelopes such as
// `{ "agreement": SlaAgreement | null }` always do. This is the discriminator
// between a result that stands alone and one that is incomplete without the
// record shape declared in its category preamble.
function recordTypesIn(text) {
  const withoutComments = text.replace(/\/\/[^\n]*/g, "");
  const withoutStrings = withoutComments.replace(/"[^"]*"/g, '""');
  return [...new Set([...withoutStrings.matchAll(/\b[A-Z][A-Za-z0-9]*\b/g)].map((m) => m[0]))].sort();
}

// A category preamble's fence is the whole result when the method declares no
// result of its own (the signal queries), and the per-record shape when the
// method declares an envelope that names a record type. That distinction is
// structural, so no prose has to be interpreted to tell the two apart.
function parseResult(block, lines, inFence, known, name, categoryShape, categoryTitle) {
  if (!block) {
    if (!categoryShape) return null;
    return {
      kind: "categoryResult",
      envelope: categoryShape,
      envelopeLang: "jsonc",
      recordTypes: [],
      recordShape: null,
      inheritedFrom: categoryTitle,
    };
  }
  const inline = (block.inline ?? "").trim();

  const alias = inline ? resolveAlias(inline, known, `${name} result`) : null;
  if (alias) return { kind: "alias", alias, note: inline || null, nullable: block.qualifier ?? null };

  const fence = firstFence(lines, inFence, block.start + 1, block.end);
  if (fence && inline) {
    fail(`${name}: result block carries both inline text and a fence, so which one is the shape is ambiguous`);
  }

  let envelope = null;
  let envelopeLang = null;
  let kind = null;
  let note = null;
  if (fence) {
    envelope = fence.body;
    envelopeLang = fence.lang;
    kind = "fence";
  } else if (inline) {
    // An inline result is prose carrying a code span, as in
    // `{ "payments": [PaymentRecord, ...] }` using the shape above.
    // The shape is the span; the rest is a note, and keeping them apart is
    // what lets the page render the shape as code rather than as a sentence.
    const span = /`([^`]+)`/.exec(inline);
    if (!span) fail(`${name}: inline result carries no code span, so no shape can be read from it`);
    envelope = span[1];
    note = inline;
    kind = "inline";
  } else {
    fail(`${name}: result block is empty`);
  }

  const recordTypes = recordTypesIn(envelope);
  if (recordTypes.length > 0 && !categoryShape) {
    fail(`${name}: the result envelope names ${recordTypes.join(", ")} but the category "${categoryTitle}" declares no record shape, so the type would render undefined`);
  }
  return {
    kind,
    envelope,
    envelopeLang,
    note,
    recordTypes,
    recordShape: recordTypes.length > 0 ? categoryShape : null,
    inheritedFrom: recordTypes.length > 0 ? categoryTitle : null,
    nullable: block.qualifier ?? null,
  };
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

function readmeMethods(root) {
  const rel = "README.md";
  const text = readText(root, rel);
  const names = new Set();
  for (const line of text.split("\n")) {
    const m = /^\|\s*`(novai_[A-Za-z]+)`\s*\|/.exec(line);
    if (m) names.add(m[1]);
  }
  if (names.size === 0) fail(`${rel}: zero rows matched in the RPC method table`);
  return { names, source: rel };
}

// Blank out Python comments and triple-quoted strings, keeping single-quoted
// and double-quoted string literals. Docstrings are triple-quoted, so a method
// merely NAMED in prose cannot count as a call site, while a name passed as an
// argument still does.
function stripPython(src, label) {
  let out = "";
  let i = 0;
  const n = src.length;
  const blank = (s) => s.replace(/[^\n]/g, " ");
  while (i < n) {
    const three = src.slice(i, i + 3);
    if (three === '"""' || three === "'''") {
      const end = src.indexOf(three, i + 3);
      if (end === -1) fail(`${label}: unterminated triple-quoted string`);
      out += blank(src.slice(i, end + 3));
      i = end + 3;
      continue;
    }
    const c = src[i];
    if (c === "#") {
      let j = i;
      while (j < n && src[j] !== "\n") j += 1;
      out += blank(src.slice(i, j));
      i = j;
      continue;
    }
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && src[j] !== "\n") {
        if (src[j] === "\\") { j += 2; continue; }
        if (src[j] === c) break;
        j += 1;
      }
      if (src[j] !== c) { out += c; i += 1; continue; }
      out += src.slice(i, j + 1);
      i = j + 1;
      continue;
    }
    out += c;
    i += 1;
  }
  if (out.length !== src.length) fail(`${label}: python scanner did not preserve length`);
  return out;
}

// Source 4: the methods the SDK actually calls. This is the strongest of the
// four because it is executable code rather than prose. Four methods reach the
// wire through a shared private helper that takes the method name as an
// argument (_list_slas, _list_channels), so matching only on `.call("novai_...")`
// would report them as missing and raise a drift that does not exist. The rule
// is therefore: a method-name literal in executable Python, docstrings and
// comments removed.
function sdkMethods(root) {
  const dir = join(root, "sdk", "novai-python-sdk", "novai_sdk");
  if (!existsSync(dir)) fail(`sdk/novai-python-sdk/novai_sdk not found under root ${root}`);
  const names = new Set();
  const files = [];
  let sawDispatcher = false;
  for (const f of walkFiles(dir, ".py")) {
    const rel = relative(root, f);
    files.push(rel);
    const src = stripPython(readFileSync(f, "utf8"), rel);
    for (const m of src.matchAll(/"(novai_[A-Za-z]+)"/g)) names.add(m[1]);
    if (/\.call\(/.test(src)) sawDispatcher = true;
  }
  if (names.size === 0) fail("sdk/novai-python-sdk: zero RPC method literals matched in executable Python");
  if (!sawDispatcher) fail("sdk/novai-python-sdk: no .call( dispatcher found, so the method literals may not reach the wire");
  return { names, source: "sdk/novai-python-sdk/novai_sdk", files: files.sort() };
}

function setDiff(a, b) {
  return [...a].filter((x) => !b.has(x)).sort();
}

// ---------------------------------------------------------------------------
// KNOWN_DRIFT
//
// Each entry carries a predicate over measured facts. The gate fails when new
// drift appears AND when a listed entry's predicate stops holding, naming the
// entry to delete. The list can only shrink.
// ---------------------------------------------------------------------------

const KNOWN_DRIFT = [
  {
    id: "error-code-32014-undocumented",
    operatorRef: "NEEDS-OPERATOR.md item 8",
    summary: "-32014 NonceTooHigh is emitted by the node and appears nowhere in the RPC reference",
    why:
      "Client-breaking rather than cosmetic. A client following the documented codes treats an " +
      "unknown rejection as terminal and resyncs, when the correct handling for NonceTooHigh is to " +
      "retry the same transaction unchanged once the sender's earlier nonces commit.",
    codes: [-32014],
    holds: (f) => f.emittedErrorCodes.includes(-32014) && !f.documentedErrorCodes.includes(-32014),
  },
  {
    id: "getnonce-documented-as-interchangeable",
    operatorRef: "NEEDS-OPERATOR.md item 15",
    summary:
      "novai_getNonce is documented as a cheaper substitute for novai_getBalance, but the two read different sources",
    why:
      "Client-breaking rather than cosmetic. getNonce answers from the mempool admission cursor and getBalance " +
      "answers from the committed account row. They agree until a committed-but-failed transaction from that " +
      "sender, after which the cursor runs ahead until the node restarts and reseeds from state. A client that " +
      "builds plain-account transactions from getNonce, as the wording invites, then signs a nonce that " +
      "execution will not accept.",
    holds: (f) => f.getNonceDocClaimsInterchangeable && f.getNonceReadsMempoolCursor,
  },
  {
    id: "public-faucet-gating-backwards",
    operatorRef: "NEEDS-OPERATOR.md item 9",
    summary: "GET /faucet/<address> is documented as dev-mode only, but the handler gates on --faucet-key",
    why:
      "Backwards in both directions. The route runs in production when --faucet-key is set, and does " +
      "NOT run on a plain --dev-keys devnet, which is the opposite of what the transport section says.",
    holds: (f) => f.publicFaucetGatesOnKeyOnly && f.httpRouteDocClaimsDevMode,
  },
  {
    id: "faucet-rpc-gating-incomplete",
    operatorRef: "NEEDS-OPERATOR.md item 13",
    summary: "novai_faucet is documented as dev-keys only, but the handler also accepts --faucet-key",
    why:
      "The same error as the HTTP route, on the JSON-RPC surface. handle_faucet prefers a loaded " +
      "--faucet-key and only falls back to the deterministic dev key, so the method runs on a " +
      "production node with a faucet key set. A reader of the doc concludes the method is " +
      "self-limiting to devnets when it is not.",
    holds: (f) => f.faucetRpcAcceptsFaucetKey && f.faucetDocClaimsDevKeysOnly,
  },
  {
    id: "faucet-disabled-code-mismatch",
    operatorRef: "NEEDS-OPERATOR.md item 10",
    summary: "novai_faucet's disabled path returns -32000, but the method's error table attributes it to -32602",
    why:
      "A client matching on -32602 to distinguish a malformed address from a disabled faucet gets the " +
      "wrong branch, and -32000 is a broad application-error code it cannot safely special-case.",
    holds: (f) => f.faucetDisabledCode === -32000 && f.faucetDocDevModeCode === -32602,
  },
];

function measureDriftFacts(root, doc, methodsByName) {
  const rel = "crates/node/src/rpc.rs";
  const { code } = scanRust(readFileSync(join(root, rel), "utf8"), rel);

  const emitted = [...new Set([...code.matchAll(/-3\d{4}/g)].map((m) => Number(m[0])))].sort((a, b) => b - a);

  const docText = readText(root, "docs/RPC_REFERENCE.md");
  const documented = [...new Set([...docText.matchAll(/`(-3\d{4})`/g)].map((m) => Number(m[1])))].sort((a, b) => b - a);

  // handle_public_faucet's signature: does it take a dev-mode flag at all?
  const sigStart = code.indexOf("fn handle_public_faucet(");
  if (sigStart === -1) fail(`${rel}: fn handle_public_faucet not found`);
  const sigEnd = code.indexOf(")", sigStart);
  const signature = code.slice(sigStart, sigEnd);
  const publicFaucetGatesOnKeyOnly = !/dev_keys/.test(signature);

  // The transport section's claim about that route.
  const httpRouteLine = doc.lines.find((l) => l.startsWith("**HTTP route (not JSON-RPC)**"));
  if (!httpRouteLine) fail("docs/RPC_REFERENCE.md: the HTTP route note was not found");
  const httpRouteDocClaimsDevMode = /dev-mode/i.test(httpRouteLine);

  // handle_faucet's disabled branch.
  const fnStart = code.indexOf("fn handle_faucet(");
  if (fnStart === -1) fail(`${rel}: fn handle_faucet not found`);
  const disabled = code.indexOf('"Faucet disabled', fnStart);
  if (disabled === -1) fail(`${rel}: the faucet disabled branch was not found in handle_faucet`);
  const before = code.slice(fnStart, disabled);
  const codeMatches = [...before.matchAll(/code:\s*(-?\d+)/g)];
  if (codeMatches.length === 0) fail(`${rel}: no error code precedes the faucet disabled message`);
  const faucetDisabledCode = Number(codeMatches[codeMatches.length - 1][1]);

  // What the doc's faucet error list attributes to the dev-mode condition.
  const faucet = methodsByName.get("novai_faucet");
  if (!faucet) fail("docs/RPC_REFERENCE.md: novai_faucet section not found");
  if (!faucet.errors || faucet.errors.kind !== "table") fail("novai_faucet: expected an errors table");
  const devRow = faucet.errors.list.find((e) => /dev-mode/i.test(e.when ?? ""));
  if (!devRow) fail("novai_faucet: no error row mentions dev-mode");

  // The JSON-RPC faucet's own gating, which is a different surface from the
  // HTTP route above and drifts the same way.
  const fnEnd = code.indexOf("\nfn ", fnStart + 1);
  const faucetBody = code.slice(fnStart, fnEnd === -1 ? code.length : fnEnd);
  const faucetRpcAcceptsFaucetKey = /if let Some\(ref [a-z_]+\) = faucet_key/.test(faucetBody);
  if (!faucetRpcAcceptsFaucetKey && !/dev_keys/.test(faucetBody)) {
    fail(`${rel}: handle_faucet resolves its key by neither faucet_key nor dev_keys; the gating check needs rewriting`);
  }
  const faucetDocClaimsDevKeysOnly =
    /--dev-keys/.test(faucet.description) && !/--faucet-key/.test(faucet.description);

  // novai_getNonce against novai_getBalance. Two independent measurements:
  // what the document invites the reader to do, and what the handler actually
  // reads. The exception holds only while both are true, so fixing either the
  // wording or the handler retires it.
  const getNonce = methodsByName.get("novai_getNonce");
  if (!getNonce) fail("docs/RPC_REFERENCE.md: novai_getNonce section not found");
  const getNonceDocClaimsInterchangeable =
    /cheaper than\s+`?getBalance`?/i.test(getNonce.description) ||
    /instead of\s+`?getBalance`?/i.test(getNonce.description);

  const nonceFnStart = code.indexOf("fn handle_get_nonce(");
  if (nonceFnStart === -1) fail(`${rel}: fn handle_get_nonce not found`);
  const nonceFnEnd = code.indexOf("\nfn ", nonceFnStart + 1);
  const nonceBody = code.slice(nonceFnStart, nonceFnEnd === -1 ? code.length : nonceFnEnd);
  const getNonceReadsMempoolCursor = /nonce_provider\s*\.\s*expected_nonce/.test(nonceBody);

  // getBalance must still be reading the committed row, or the contrast this
  // exception describes is no longer the contrast that exists.
  const balanceFnStart = code.indexOf("fn handle_get_balance(");
  if (balanceFnStart === -1) fail(`${rel}: fn handle_get_balance not found`);
  const balanceFnEnd = code.indexOf("\nfn ", balanceFnStart + 1);
  const balanceBody = code.slice(balanceFnStart, balanceFnEnd === -1 ? code.length : balanceFnEnd);
  if (getNonceReadsMempoolCursor && !/read_account_or_default/.test(balanceBody)) {
    fail(
      `${rel}: handle_get_nonce reads the mempool cursor but handle_get_balance no longer reads the account row, ` +
      `so the documented contrast between the two is no longer accurate and needs re-measuring`
    );
  }

  return {
    emittedErrorCodes: emitted,
    documentedErrorCodes: documented,
    getNonceDocClaimsInterchangeable,
    getNonceReadsMempoolCursor,
    publicFaucetGatesOnKeyOnly,
    httpRouteDocClaimsDevMode,
    faucetDisabledCode,
    faucetDocDevModeCode: devRow.code,
    faucetRpcAcceptsFaucetKey,
    faucetDocClaimsDevKeysOnly,
  };
}

// ---------------------------------------------------------------------------
// Enumerations from the Rust source
// ---------------------------------------------------------------------------

function enumBody(source, name, file) {
  const anchor = source.indexOf(`pub enum ${name}`);
  if (anchor === -1) fail(`enum ${name} not found in ${file}`);
  const open = source.indexOf("{", anchor);
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

// Unit variants with their leading doc comment. A variant with no doc comment
// yields description: null, which the report counts and the page must not
// silently render as complete.
function enumVariants(body, name, file) {
  const out = [];
  let doc = [];
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (t.startsWith("///")) { doc.push(t.replace(/^\/\/\/\s?/, "").trim()); continue; }
    if (t === "" || t.startsWith("//")) continue;
    if (t.startsWith("#")) continue;
    const m = /^([A-Za-z][A-Za-z0-9_]*)\s*=\s*(\d+)\s*,?$/.exec(t);
    if (m) {
      out.push({
        variant: m[1],
        discriminant: Number(m[2]),
        description: doc.length ? normaliseDashes(doc.join(" "), file) : null,
      });
    }
    doc = [];
  }
  if (out.length === 0) fail(`enum ${name}: zero unit variants matched in ${file}`);
  const sorted = [...out].sort((a, b) => a.discriminant - b.discriminant);
  sorted.forEach((v, i) => {
    if (v.discriminant !== i) fail(`enum ${name}: discriminants not contiguous from 0 (saw ${v.discriminant} at position ${i})`);
  });
  return sorted;
}

function txTypes(root) {
  const rel = "crates/execution/src/lib.rs";
  const src = readFileSync(join(root, rel), "utf8");
  const out = [...src.matchAll(/^pub const ([A-Z_]+)_PAYLOAD_V1: u8 = (\d+);$/gm)].map((m) => ({
    constant: `${m[1]}_PAYLOAD_V1`,
    name: m[1].toLowerCase().split("_").map((w, i) => (i === 0 ? w : w[0].toUpperCase() + w.slice(1))).join(""),
    discriminant: Number(m[2]),
  }));
  if (out.length === 0) fail(`${rel}: zero PAYLOAD_V1 constants matched`);
  const sorted = out.sort((a, b) => a.discriminant - b.discriminant);
  sorted.forEach((t, i) => {
    if (t.discriminant !== i + 1) fail(`${rel}: payload discriminants not contiguous from 1 (saw ${t.discriminant} at position ${i})`);
  });
  return sorted;
}

function namedConst(root, rel, name) {
  const src = readFileSync(join(root, rel), "utf8");
  const m = new RegExp(`^pub const ${name}: [A-Za-z0-9_]+ = ([0-9_]+);`, "m").exec(src);
  if (!m) fail(`${rel}: const ${name} not found`);
  return Number(m[1].replace(/_/g, ""));
}

// ---------------------------------------------------------------------------
// Source-derived datasets
//
// Everything below reads crates/ and sdk/ rather than the reference document.
// The document is one witness; these are the implementation. Where a value
// exists in both, the two are cross-checked and a disagreement fails the build.
// ---------------------------------------------------------------------------

/**
 * A constant declaration anywhere in a Rust file, with or without `pub` and
 * with or without a type annotation, so this reaches both a crate-level
 * `pub const` and a function-local `const`.
 *
 * Asserts exactly one declaration. Two would mean a test module shadows the
 * real one and the generator would be reading whichever came first, which is
 * the kind of instrument error that makes the measurement worthless.
 */
function rustConst(root, rel, name) {
  const src = readText(root, rel);
  const { code } = scanRust(src, rel);
  const re = new RegExp(
    `^[ \\t]*(?:pub[ \\t]+)?const[ \\t]+${name}[ \\t]*(?::[^=;]+)?=[ \\t]*([^;]+);`,
    "gm"
  );
  const hits = [...code.matchAll(re)];
  if (hits.length === 0) fail(`${rel}: const ${name} not found`);
  if (hits.length > 1) {
    fail(`${rel}: const ${name} is declared ${hits.length} times, so which one is read is ambiguous`);
  }
  return {
    name,
    value: evalIntExpr(hits[0][1].trim(), `${rel} ${name}`),
    file: rel,
    line: lineOf(code, hits[0].index),
  };
}

/** Every `pub const` whose name matches, with its leading doc comment. */
function rustConstsMatching(root, rel, namePattern) {
  const src = readText(root, rel);
  const { code } = scanRust(src, rel);
  const out = [];
  const re = new RegExp(`^[ \\t]*pub[ \\t]+const[ \\t]+(${namePattern})[ \\t]*(?::[^=;]+)?=[ \\t]*([^;]+);`, "gm");
  for (const m of code.matchAll(re)) {
    // The doc comment is taken from the ORIGINAL text, because scanRust strips
    // comments. Walk back over the preceding /// lines.
    const line = lineOf(code, m.index);
    const srcLines = src.split("\n");
    const doc = [];
    for (let i = line - 2; i >= 0; i--) {
      const t = srcLines[i].trim();
      if (t.startsWith("///")) { doc.unshift(t.replace(/^\/\/\/\s?/, "")); continue; }
      break;
    }
    out.push({
      name: m[1],
      value: evalIntExpr(m[2].trim(), `${rel} ${m[1]}`),
      expression: m[2].trim(),
      description: doc.length ? normaliseDashes(doc.join(" "), rel) : null,
      file: rel,
      line,
    });
  }
  if (out.length === 0) fail(`${rel}: no constants matched /${namePattern}/`);
  return out.sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * Every JSON-RPC error code the node can emit.
 *
 * Three emission forms exist and all three are scanned, because measuring only
 * the obvious one silently loses the mempool codes:
 *   1. `code: -32602` in an RpcError literal
 *   2. `(-32014, format!(...))` tuple arms in the mempool error mapping
 *   3. `"code": -32003` inside the hand-built JSON of the too-large path
 * Comments are stripped by scanRust first, so the commented code table above
 * the mempool match does not inject phantom codes.
 *
 * The real guard is not this scan but the cross-check in compute(): the set
 * found here must equal the document's table plus the codes carried in
 * KNOWN_DRIFT. A missed form therefore fails the build rather than quietly
 * shrinking the count.
 */
function errorCodesFromSource(root) {
  const rel = "crates/node/src/rpc.rs";
  const { code } = scanRust(readText(root, rel), rel);
  const fns = [...code.matchAll(/\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[(<]/g)]
    .map((m) => ({ name: m[1], index: m.index }));
  const patterns = [
    /\bcode:\s*(-3\d{4})\b/g,
    /\(\s*(-3\d{4})\s*,/g,
    /"code":\s*(-3\d{4})\b/g,
  ];
  const byCode = new Map();
  for (const re of patterns) {
    for (const m of code.matchAll(re)) {
      const value = Number(m[1]);
      let owner = null;
      for (const f of fns) {
        if (f.index < m.index) owner = f.name;
        else break;
      }
      const line = lineOf(code, m.index);
      if (!byCode.has(value)) byCode.set(value, { code: value, file: rel, line, functions: new Set() });
      const entry = byCode.get(value);
      if (line < entry.line) entry.line = line;
      if (owner) entry.functions.add(owner);
    }
  }
  if (byCode.size === 0) fail(`${rel}: zero JSON-RPC error codes matched`);

  // Prove the instrument. The three patterns above are structured, so they can
  // attach a function and a line to each code, but a structured pattern is
  // exactly the thing that silently misses a fourth emission form. A broad
  // sweep of the same comment-stripped source cannot miss one, so the two must
  // agree: if the broad sweep sees a code the structured patterns did not, a
  // form has been added and this parser is now undercounting.
  const broad = new Set([...code.matchAll(/-3\d{4}/g)].map((m) => Number(m[0])));
  const missed = [...broad].filter((c) => !byCode.has(c)).sort((a, b) => b - a);
  if (missed.length) {
    fail(
      `${rel}: error codes ${missed.join(", ")} appear in the source but match none of the known emission ` +
      `forms, so the structured scan is undercounting. Add the new form to errorCodesFromSource.`
    );
  }

  return [...byCode.values()]
    .sort((a, b) => b.code - a.code)
    .map((e) => ({ code: e.code, file: e.file, line: e.line, functions: [...e.functions].sort() }));
}

/**
 * HTTP-level rejections that never carry a JSON-RPC envelope.
 *
 * These are the ones that break a client calling .json() on the response, so
 * the body literal matters as much as the status.
 */
function httpRejectionsFromSource(root) {
  const rel = "crates/node/src/rpc.rs";
  const { code } = scanRust(readText(root, rel), rel);
  const out = [];
  for (const m of code.matchAll(/StatusCode\((\d{3})\)/g)) {
    const before = code.slice(Math.max(0, m.index - 400), m.index);
    // Bodies appear both as a bare literal and wrapped in format!, so both
    // shapes are read. A format! body is reported as a template rather than as
    // a fixed string, because that is what a client actually receives.
    const body = [...before.matchAll(/from_string\(\s*(?:format!\(\s*)?"([^"]*)"/g)].pop();
    const templated = /from_string\(\s*format!\(/.test(before.slice(before.lastIndexOf("from_string(")));
    out.push({
      status: Number(m[1]),
      body: body ? body[1] : null,
      bodyIsTemplate: body ? templated : null,
      file: rel,
      line: lineOf(code, m.index),
    });
  }
  const byStatus = new Map();
  for (const r of out) if (!byStatus.has(r.status)) byStatus.set(r.status, r);
  for (const want of [400, 413, 429, 503]) {
    if (!byStatus.has(want)) fail(`${rel}: expected an HTTP ${want} rejection site and found none`);
    if (byStatus.get(want).body === null) {
      fail(`${rel}: the HTTP ${want} rejection has no readable body, so its documented shape cannot be generated`);
    }
  }
  return [...byStatus.values()].sort((a, b) => a.status - b.status);
}

/** The six RPC limits, read from the constants rather than from the doc table. */
function limitsFromSource(root) {
  const rpc = "crates/node/src/rpc.rs";
  return [
    { ...rustConst(root, rpc, "MAX_RPC_REQUESTS_PER_SEC"), unit: "requests per second per source IP" },
    { ...rustConst(root, rpc, "MAX_CONCURRENT_RPC"), unit: "concurrent in-flight requests" },
    { ...rustConst(root, rpc, "MAX_RPC_BODY_SIZE"), unit: "bytes, request body" },
    { ...rustConst(root, rpc, "MAX_RPC_RESPONSE_SIZE"), unit: "bytes, response body" },
    { ...rustConst(root, "crates/types/src/lib.rs", "MAX_TX_SIZE"), unit: "bytes, decoded transaction" },
    { ...rustConst(root, rpc, "MAX_SIGNAL_QUERY_RANGE"), unit: "blocks, signal query height range" },
  ];
}

/**
 * The minimum fee for every transaction type, from the dispatch in
 * minimum_fee_for_tx rather than from the cookbook's table, which lists seven
 * operations and omits two of the eleven types.
 */
function feesFromSource(root, payloads) {
  const rel = "crates/execution/src/lib.rs";
  const { code } = scanRust(readText(root, rel), rel);
  const anchor = /pub fn minimum_fee_for_tx\s*\([^)]*\)[^{]*\{/.exec(code);
  if (!anchor) fail(`${rel}: minimum_fee_for_tx not found`);
  const matchIdx = code.indexOf("match version {", anchor.index);
  if (matchIdx === -1) fail(`${rel}: the fee match block was not found inside minimum_fee_for_tx`);
  const open = code.indexOf("{", matchIdx + "match version".length);
  let depth = 0;
  let close = -1;
  for (let i = open; i < code.length; i++) {
    if (code[i] === "{") depth += 1;
    else if (code[i] === "}") { depth -= 1; if (depth === 0) { close = i; break; } }
  }
  if (close === -1) fail(`${rel}: unbalanced braces in the fee match block`);
  const body = code.slice(open + 1, close);

  // Arms may carry or-patterns, which is how the three memory-object types
  // share one fee. Splitting on => and then on | covers both shapes.
  const feeOf = new Map();
  for (const arm of body.split(/,\s*(?=[A-Z_]|_\s*=>)/)) {
    const m = /^([\s\S]*?)=>\s*Ok\(\s*([A-Z_][A-Z0-9_]*)\s*\)/.exec(arm.trim());
    if (!m) continue;
    const feeConst = m[2];
    for (const raw of m[1].split("|")) {
      const name = raw.trim();
      if (!/^[A-Z][A-Z0-9_]*$/.test(name)) continue;
      feeOf.set(name, feeConst);
    }
  }
  if (feeOf.size === 0) fail(`${rel}: zero fee arms parsed from minimum_fee_for_tx`);

  const out = payloads.map((t) => {
    const feeConst = feeOf.get(t.constant);
    if (!feeConst) {
      fail(`${rel}: transaction type ${t.constant} has no arm in minimum_fee_for_tx, so its fee cannot be generated`);
    }
    const c = rustConst(root, rel, feeConst);
    return { ...t, feeConstant: feeConst, minFee: c.value, feeLine: c.line, feeFile: rel };
  });
  return out;
}

/** Percentage fees, expressed in basis points against a shared denominator. */
function bpsFeesFromSource(root) {
  const rel = "crates/execution/src/lib.rs";
  const denominator = rustConst(root, rel, "BPS_DENOMINATOR");
  const names = ["PAYMENT_FEE_BPS", "MARKETPLACE_FEE_BPS", "SUBSCRIPTION_CANCEL_FEE_BPS"];
  return {
    denominator: denominator.value,
    denominatorConstant: denominator.name,
    entries: names.map((n) => {
      const c = rustConst(root, rel, n);
      return { constant: c.name, bps: c.value, percent: (c.value / denominator.value) * 100, file: c.file, line: c.line };
    }),
  };
}

/**
 * The canonical unsigned transaction encoding, walked out of the encoder.
 *
 * This is the artifact a non-Rust client most needs and most easily gets
 * wrong, because the envelope is little-endian while payload internals are
 * big-endian.
 */
const WIRE_WRITERS = new Map([
  ["write_u8", { bytes: 1, endianness: null }],
  ["write_32", { bytes: 32, endianness: null }],
  ["write_u32_le", { bytes: 4, endianness: "little" }],
  ["write_u64_le", { bytes: 8, endianness: "little" }],
]);

function txWireLayout(root) {
  const rel = "crates/codec/src/lib.rs";
  const { code } = scanRust(readText(root, rel), rel);
  const anchor = /pub fn encode_tx_v1_unsigned\s*\([^)]*\)[^{]*\{/.exec(code);
  if (!anchor) fail(`${rel}: encode_tx_v1_unsigned not found`);
  let depth = 0;
  let close = -1;
  const open = anchor.index + anchor[0].length - 1;
  for (let i = open; i < code.length; i++) {
    if (code[i] === "{") depth += 1;
    else if (code[i] === "}") { depth -= 1; if (depth === 0) { close = i; break; } }
  }
  if (close === -1) fail(`${rel}: unbalanced braces in encode_tx_v1_unsigned`);
  const body = code.slice(open + 1, close);

  const fieldName = (arg) =>
    arg.trim()
      .replace(/^&/, "")
      .replace(/^tx\./, "")
      .replace(/\s+as\s+\w+$/, "")
      .replace(/\.len\(\)$/, "_len")
      .trim();

  const fields = [];
  let offset = 0;
  for (const line of body.split("\n")) {
    // Greedy to the final close paren: the payload-length argument is
    // `tx.payload.len() as u32`, which contains a paren of its own, so a
    // non-greedy [^)] class stops early, matches nothing, and drops the field
    // without saying so. The overhead assertion below exists because that is
    // exactly what happened.
    const w = /^\s*(write_[a-z0-9_]+)\(\s*&mut out\s*,\s*(.+)\)\s*;\s*$/.exec(line);
    if (w) {
      const spec = WIRE_WRITERS.get(w[1]);
      if (!spec) fail(`${rel}: unknown wire writer ${w[1]} in encode_tx_v1_unsigned`);
      const name = fieldName(w[2]);
      if (!/^[a-z][a-z0-9_]*$/.test(name)) fail(`${rel}: could not resolve a field name from "${w[2]}"`);
      fields.push({ field: name, bytes: spec.bytes, endianness: spec.endianness, offset });
      offset += spec.bytes;
      continue;
    }
    const ext = /^\s*out\.extend_from_slice\(\s*(.+)\)\s*;\s*$/.exec(line);
    if (ext) {
      const name = fieldName(ext[1]);
      if (!/^[a-z][a-z0-9_]*$/.test(name)) fail(`${rel}: could not resolve a field name from "${ext[1]}"`);
      fields.push({ field: name, bytes: null, endianness: null, offset });
      offset = null;
    }
  }
  if (fields.length === 0) fail(`${rel}: zero fields parsed from encode_tx_v1_unsigned`);
  if (!fields.some((f) => f.bytes === null)) {
    fail(`${rel}: expected a variable-length payload field in encode_tx_v1_unsigned and found none`);
  }
  // The signature width, read from the signed encoder rather than assumed.
  const signedAnchor = /pub fn encode_tx_v1_signed\s*\([^)]*\)[^{]*\{/.exec(code);
  if (!signedAnchor) fail(`${rel}: encode_tx_v1_signed not found`);
  const signedBody = code.slice(signedAnchor.index, code.indexOf("\n}", signedAnchor.index));
  const sigWrite = /write_(\d+)\(\s*&mut out\s*,\s*&tx\.sig\s*\)/.exec(signedBody);
  if (!sigWrite) fail(`${rel}: the signature write in encode_tx_v1_signed could not be read`);
  const signatureBytes = Number(sigWrite[1]);

  // Arithmetic check on the parsed layout.
  //
  // TX_V1_OVERHEAD is everything except the payload, so the fixed fields
  // walked out of the encoder plus the signature must add up to it exactly. A
  // field that the line parser silently failed to match makes this sum come up
  // short, which is how a dropped field becomes a build failure instead of a
  // wrong byte offset published as a signing spec.
  const overhead = rustConst(root, rel, "TX_V1_OVERHEAD");
  const fixedSum = fields.reduce((a, f) => a + (f.bytes ?? 0), 0);
  if (fixedSum + signatureBytes !== overhead.value) {
    fail(
      `${rel}: the fields parsed from encode_tx_v1_unsigned sum to ${fixedSum} bytes and the signature is ` +
      `${signatureBytes}, totalling ${fixedSum + signatureBytes}, but ${overhead.name} is ${overhead.value}. ` +
      `A field was missed or the encoding changed; the published signing spec would be wrong either way. ` +
      `Parsed: ${fields.map((f) => `${f.field}(${f.bytes ?? "var"})`).join(", ")}.`
    );
  }

  return {
    fields,
    signatureBytes,
    overhead: overhead.value,
    overheadConstant: overhead.name,
    file: rel,
    line: lineOf(code, anchor.index),
  };
}

/** The capability bits, read from the bit tests rather than from the table. */
function capabilityBits(root) {
  const rel = "crates/ai_entities/src/lib.rs";
  const { code } = scanRust(readText(root, rel), rel);
  // Anchor on the impl block, not on the bare function name: AutonomyMode
  // declares a from_byte too and it comes first in the file, so searching for
  // the function alone reads the wrong body and finds no bits.
  const implStart = code.indexOf("impl Capabilities {");
  if (implStart === -1) fail(`${rel}: impl Capabilities block not found`);
  let depth = 0;
  let implEnd = -1;
  const braceAt = code.indexOf("{", implStart);
  for (let i = braceAt; i < code.length; i++) {
    if (code[i] === "{") depth += 1;
    else if (code[i] === "}") { depth -= 1; if (depth === 0) { implEnd = i; break; } }
  }
  if (implEnd === -1) fail(`${rel}: unbalanced braces in impl Capabilities`);
  const anchor = code.indexOf("pub fn from_byte", implStart);
  if (anchor === -1 || anchor > implEnd) fail(`${rel}: Capabilities::from_byte not found`);
  const region = code.slice(anchor, implEnd);
  const out = [];
  for (const m of region.matchAll(/([a-z_][a-z0-9_]*)\s*:\s*\(\s*byte\s*&\s*\(\s*1\s*<<\s*(\d+)\s*\)\s*\)\s*!=\s*0/g)) {
    out.push({ capability: m[1], bit: Number(m[2]), hex: `0x${(1 << Number(m[2])).toString(16).padStart(2, "0")}` });
  }
  if (out.length === 0) fail(`${rel}: zero capability bits parsed from from_byte`);
  return out.sort((a, b) => a.bit - b.bit);
}

/**
 * The quorum rule, read from BOTH const fn sites and asserted identical.
 *
 * Two independent implementations of the same formula is itself the gate: if
 * one is edited and the other is not, this fails rather than publishing a rule
 * that only half the code obeys.
 */
function quorumRule(root) {
  const sites = [
    { file: "crates/node/src/snapshot/valset.rs", fn: "quorum" },
    { file: "crates/consensus_types/src/leader.rs", fn: "quorum_threshold" },
  ];
  const found = sites.map((s) => {
    const { code } = scanRust(readText(root, s.file), s.file);
    const anchor = new RegExp(`pub const fn ${s.fn}\\s*\\([^)]*\\)[^{]*\\{`).exec(code);
    if (!anchor) fail(`${s.file}: const fn ${s.fn} not found`);
    let depth = 0;
    let close = -1;
    const open = anchor.index + anchor[0].length - 1;
    for (let i = open; i < code.length; i++) {
      if (code[i] === "{") depth += 1;
      else if (code[i] === "}") { depth -= 1; if (depth === 0) { close = i; break; } }
    }
    if (close === -1) fail(`${s.file}: unbalanced braces in ${s.fn}`);
    const tail = code.slice(open + 1, close).split("\n").map((l) => l.trim()).filter(Boolean);
    const expr = tail[tail.length - 1];
    if (!expr) fail(`${s.file}: ${s.fn} has an empty body`);
    return { ...s, expression: expr.replace(/\s+/g, " "), line: lineOf(code, anchor.index) };
  });
  if (found[0].expression !== found[1].expression) {
    fail(
      `the quorum rule differs between its two sites: ` +
      `${found[0].file} has "${found[0].expression}" and ${found[1].file} has "${found[1].expression}"`
    );
  }
  return { expression: found[0].expression, sites: found };
}

/**
 * SDK coverage across the three SDKs.
 *
 * Names are compared on a canonical key rather than literally, because the
 * builders drop the word "object" from the memory types. An unmatched
 * transaction type is reported as missing rather than skipped.
 */
/**
 * Split a source file into named function bodies, given a regex whose first
 * capture is the function name. A body runs to the next declaration.
 */
function functionBodies(src, declRe) {
  const decls = [...src.matchAll(declRe)].map((m) => ({ name: m[1], index: m.index }));
  return decls.map((d, i) => ({
    name: d.name,
    body: src.slice(d.index, i + 1 < decls.length ? decls[i + 1].index : src.length),
  }));
}

/** Members of a Python IntEnum, as name to value. */
function pyEnumMembers(src, className, where) {
  const lines = src.split("\n");
  const start = lines.findIndex((l) => new RegExp(`^class ${className}\\(IntEnum\\):`).test(l));
  if (start === -1) fail(`${where}: class ${className} not found`);
  const out = new Map();
  for (let i = start + 1; i < lines.length; i++) {
    const l = lines[i];
    if (l.trim() === "") continue;
    if (!/^\s/.test(l)) break; // dedent to column zero ends the class
    const m = /^\s+([A-Z][A-Z0-9_]*)\s*=\s*(\d+)/.exec(l);
    if (m) out.set(m[1], Number(m[2]));
  }
  if (out.size === 0) fail(`${where}: class ${className} has no integer members`);
  return out;
}

function sdkCoverage(root, payloads, signalCount, memoryCount) {
  // Builders are matched to transaction types by the DISCRIMINANT THEY EMIT,
  // not by their name. The three SDKs name the same builder three different
  // ways (register_ai_entity, registerAiEntity, build_register_entity_payload),
  // so name matching needs a per-SDK convention table that silently reports a
  // renamed builder as a missing one. The first payload byte is the thing that
  // actually decides which transaction type is being built, and it cannot be
  // renamed.
  const rustBodies = functionBodies(readText(root, "sdk/novai-sdk/src/tx.rs"), /^pub fn ([a-z_][a-z0-9_]*)\s*\(/gm);
  const rustByCode = new Map();
  for (const f of rustBodies) {
    const m = /payload\.push\((\d+)\)/.exec(f.body);
    if (m) rustByCode.set(Number(m[1]), f.name);
  }

  const tsBodies = functionBodies(readText(root, "sdk/novai-sdk-ts/src/tx.ts"), /^export function ([A-Za-z_][A-Za-z0-9_]*)\s*\(/gm);
  const tsByCode = new Map();
  for (const f of tsBodies) {
    const m = /payload\[0\]\s*=\s*(\d+)\s*;/.exec(f.body);
    if (m) tsByCode.set(Number(m[1]), f.name);
  }

  const pyEnumsSrc = readText(root, "sdk/novai-python-sdk/novai_sdk/enums.py");
  const pyPayloadTypes = pyEnumMembers(pyEnumsSrc, "TxPayloadType", "sdk/novai-python-sdk/novai_sdk/enums.py");
  const pyByCode = new Map();
  for (const f of walkFiles(join(root, "sdk/novai-python-sdk/novai_sdk/tx"), ".py")) {
    const rel = relative(root, f);
    for (const fn of functionBodies(readText(root, rel), /^def (build_[a-z0-9_]+_payload)\s*\(/gm)) {
      const m = /TxPayloadType\.([A-Z][A-Z0-9_]*)/.exec(fn.body);
      if (!m) continue;
      const value = pyPayloadTypes.get(m[1]);
      if (value === undefined) fail(`${rel}: ${fn.name} names TxPayloadType.${m[1]}, which is not a member of that enum`);
      pyByCode.set(value, fn.name);
    }
  }

  for (const [label, map] of [["Rust", rustByCode], ["TypeScript", tsByCode], ["Python", pyByCode]]) {
    if (map.size === 0) fail(`the ${label} SDK: zero transaction builders were matched to a payload discriminant`);
  }

  const builders = payloads.map((t) => ({
    txType: t.name,
    discriminant: t.discriminant,
    rust: rustByCode.get(t.discriminant) ?? null,
    typescript: tsByCode.get(t.discriminant) ?? null,
    python: pyByCode.get(t.discriminant) ?? null,
  }));

  // Enum coverage. The Rust SDK re-exports the chain's own enums, so its
  // coverage is the chain's by construction; the TypeScript SDK redeclares
  // them, which is exactly why it can drift. That distinction is asserted
  // rather than assumed.
  const rustLib = readText(root, "sdk/novai-sdk/src/lib.rs");
  const rustReexports = /pub use novai_ai_entities::\{[^}]*\bAiSignalType\b[^}]*\bMemoryObjectType\b[^}]*\}/s.test(rustLib);
  if (!rustReexports) {
    fail("sdk/novai-sdk/src/lib.rs: the Rust SDK no longer re-exports AiSignalType and MemoryObjectType from novai_ai_entities, so its type coverage can no longer be stated as structural");
  }
  const tsTypes = readText(root, "sdk/novai-sdk-ts/src/types.ts");
  const enumCount = (name) => {
    const m = new RegExp(`export enum ${name}\\s*\\{([^}]*)\\}`, "s").exec(tsTypes);
    if (!m) fail(`sdk/novai-sdk-ts/src/types.ts: enum ${name} not found`);
    return [...m[1].matchAll(/^\s*[A-Za-z][A-Za-z0-9_]*\s*=\s*\d+\s*,?\s*$/gm)].length;
  };

  const pyEnumCount = (name) => pyEnumMembers(pyEnumsSrc, name, "sdk/novai-python-sdk/novai_sdk/enums.py").size;

  return {
    builders,
    totals: {
      txTypes: payloads.length,
      rustBuilders: builders.filter((b) => b.rust).length,
      typescriptBuilders: builders.filter((b) => b.typescript).length,
      pythonBuilders: builders.filter((b) => b.python).length,
    },
    signalTypes: {
      chain: signalCount,
      rust: signalCount,
      rustIsStructural: true,
      typescript: enumCount("SignalType"),
      python: pyEnumCount("AiSignalType"),
    },
    memoryObjectTypes: {
      chain: memoryCount,
      rust: memoryCount,
      rustIsStructural: true,
      typescript: enumCount("MemoryObjectType"),
      python: pyEnumCount("MemoryObjectType"),
    },
    // Only the Rust case is derivable from the tree: a manifest with path
    // dependencies on the workspace cannot be consumed from a registry, and
    // that is a fact about these files. Whether a package is PUBLISHED is an
    // external fact that no amount of reading this repo can establish, so it
    // is deliberately absent here and hand-written with its check date in the
    // section-07 copy instead. Emitting it here would dress an operator
    // observation up as a generated one.
    workspaceCoupling: {
      rust: rustPathDeps(root),
    },
  };
}

/**
 * Path dependencies in the Rust SDK's manifest. A crate depending on
 * `path = "../../crates/..."` cannot be published or consumed outside a clone
 * of this repository, which is the real constraint on the Rust install line.
 */
function rustPathDeps(root) {
  const rel = "sdk/novai-sdk/Cargo.toml";
  const src = readText(root, rel);
  const deps = [...src.matchAll(/^([A-Za-z0-9_-]+)\s*=\s*\{[^}]*\bpath\s*=\s*"([^"]+)"/gm)]
    .map((m) => ({ crate: m[1], path: m[2] }));
  return { file: rel, pathDependencies: deps, consumableFromRegistry: deps.length === 0 };
}

// ---------------------------------------------------------------------------
// OpenRPC
// ---------------------------------------------------------------------------

const TYPE_MAP = new Map([
  ["u8", { type: "integer", minimum: 0, maximum: 255 }],
  ["u16", { type: "integer", minimum: 0, maximum: 65535 }],
  ["u64", { type: "integer", minimum: 0 }],
  ["hex32", { type: "string", pattern: "^[0-9a-fA-F]{64}$" }],
  ["hex", { type: "string", pattern: "^[0-9a-fA-F]*$" }],
  ["string", { type: "string" }],
]);

function schemaFor(docType, context) {
  const s = TYPE_MAP.get(docType);
  if (!s) fail(`${context}: no schema mapping for the documented type "${docType}"; add one deliberately rather than defaulting to string`);
  return { ...s, "x-novai-doc-type": docType };
}

function buildOpenRpc(methods, endpointNote) {
  return {
    openrpc: "1.2.6",
    info: {
      title: "NOVAI JSON-RPC",
      version: "1.0.0",
      description:
        "Generated from docs/RPC_REFERENCE.md and cross-checked against the node's dispatch table, " +
        "the README method table and the Python SDK. Not hand-maintained.",
    },
    methods: methods.map((m) => {
      const params = m.params && m.params.kind === "table" ? m.params.list : [];
      return {
        name: m.name,
        summary: m.brief,
        description: m.description || undefined,
        params: params.map((p) => ({
          name: p.field,
          required: !p.optional,
          schema: schemaFor(p.type, `${m.name}.${p.field}`),
          description: p.notes || undefined,
        })),
        result: {
          name: "result",
          schema: { type: ["object", "null"] },
          description: m.resultSummary || undefined,
        },
        errors: (m.errorList ?? []).map((e) => ({ code: e.code, message: e.when || "see the reference" })),
        // Provenance travels with the description, so a reader of this file
        // can tell a shape the method declares from one it inherits.
        "x-novai-params-source": m.params ? (m.params.resolvedFrom ? `alias of ${m.params.resolvedFrom}` : m.params.kind) : "absent",
        "x-novai-result-source": m.result ? (m.result.resolvedFrom ? `alias of ${m.result.resolvedFrom}` : m.result.kind) : "absent",
        "x-novai-result-record-shape-from": m.result?.inheritedFrom ?? undefined,
      };
    }),
    "x-novai-endpoint-note": endpointNote,
  };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function compute(root) {
  const docText = readText(root, "docs/RPC_REFERENCE.md");
  const doc = parseDoc(docText);
  const { lines, inFence, categories, methods } = doc;

  const docNames = new Set(methods.map((m) => m.name));

  // Category preambles: the shared record shape and any Common errors table.
  const categoryData = new Map();
  for (const c of categories) {
    const preambleEnd = c.methods.length > 0 ? methods.find((m) => m.name === c.methods[0]).start : c.end;
    const blocks = blocksOf(lines, inFence, c.start + 1, preambleEnd);
    const shapeFence = firstFence(lines, inFence, c.start + 1, preambleEnd);
    const commonBlock = findBlock(blocks, "Common errors");
    const common = commonBlock ? parseTable(lines, inFence, commonBlock.start + 1, commonBlock.end) : null;
    const extras = {};
    for (const b of blocks) {
      if (b.prose || /^common errors$/i.test(b.label)) continue;
      const t = parseTable(lines, inFence, b.start + 1, b.end);
      if (t) extras[b.label] = t.rows;
    }
    categoryData.set(c.title, {
      shape: shapeFence && shapeFence.lang === "jsonc" ? shapeFence.body : null,
      commonErrors: common ? common.rows.map((r) => ({ code: Number(stripTicks(r.Code)), when: r.When })) : null,
      extras,
    });
  }

  // The method index: category and one-line brief per method.
  const indexBriefs = new Map();
  for (const line of lines) {
    const m = /^\|[^|]*\|\s*\[`(novai_[A-Za-z]+)`\]\([^)]*\)\s*\|\s*(.+?)\s*\|\s*$/.exec(line);
    if (m) indexBriefs.set(m[1], m[2]);
  }
  // The index is a fifth in-document list I depend on for every brief. It is
  // not part of the four-way gate, but a disagreement would silently drop a
  // brief, so it fails the generator outright.
  const indexMissing = setDiff(docNames, new Set(indexBriefs.keys()));
  const indexExtra = setDiff(new Set(indexBriefs.keys()), docNames);
  if (indexMissing.length || indexExtra.length) {
    fail(
      `docs/RPC_REFERENCE.md: the method index disagrees with the method sections. ` +
      `Missing from the index: ${indexMissing.join(", ") || "none"}. ` +
      `In the index with no section: ${indexExtra.join(", ") || "none"}.`
    );
  }

  // Per-method parse.
  const built = [];
  for (const m of methods) {
    const blocks = blocksOf(lines, inFence, m.start + 1, m.end);
    const cat = categoryData.get(m.category);

    // Description: prose between the heading and the first labelled block.
    const proseEnd = blocks.length ? blocks[0].start : m.end;
    const description = lines.slice(m.start + 1, proseEnd)
      .filter((l) => l.trim() !== "" && !l.trim().startsWith("---"))
      .join(" ").trim();

    const params = parseParams(findBlock(blocks, "Params"), lines, inFence, docNames, m.name);
    const result = parseResult(findBlock(blocks, "Result"), lines, inFence, docNames, m.name, cat.shape, m.category);
    let errors = parseErrors(findBlock(blocks, "Errors"), lines, inFence, docNames, m.name);
    if (!errors && cat.commonErrors) {
      errors = { kind: "categoryCommon", list: cat.commonErrors, from: m.category };
    }

    const exampleBlock = blocks.find((b) => /^examples?$/i.test(b.label) && !b.prose);
    if (!exampleBlock) fail(`${m.name}: no Example block`);
    const bash = fencesOfLang(lines, exampleBlock.start + 1, exampleBlock.end, "bash");
    if (bash.length === 0) fail(`${m.name}: Example block has no bash fence`);
    const curl = bash[0].body;
    const embedded = /"method"\s*:\s*"(novai_[A-Za-z]+)"/.exec(curl);
    if (!embedded) fail(`${m.name}: the curl example does not carry a method name`);
    if (embedded[1] !== m.name) {
      fail(`${m.name}: the curl example calls ${embedded[1]}, so the example under this heading is for a different method`);
    }
    const json = fencesOfLang(lines, exampleBlock.start + 1, exampleBlock.end, "json");

    built.push({
      name: m.name,
      category: m.category,
      brief: indexBriefs.get(m.name),
      description,
      params,
      result,
      errors,
      curl,
      sampleResponse: json.length ? json[0].body : null,
      exampleNote: exampleBlock.qualifier ?? null,
    });
  }

  // Resolve aliases into a flat, renderable view while keeping provenance.
  const byName = new Map(built.map((b) => [b.name, b]));
  for (const m of built) {
    if (m.params && m.params.kind === "alias") {
      const t = byName.get(m.params.alias);
      if (!t.params || t.params.kind === "alias") fail(`${m.name}: params alias chain does not terminate`);
      m.params = { ...t.params, resolvedFrom: m.params.alias, note: m.params.note };
    }
    if (m.result && m.result.kind === "alias") {
      const t = byName.get(m.result.alias);
      if (!t.result || t.result.kind === "alias") fail(`${m.name}: result alias chain does not terminate`);
      m.result = { ...t.result, resolvedFrom: m.result.alias, note: m.result.note, nullable: m.result.nullable };
    }
    if (m.errors && m.errors.kind === "alias") {
      const t = byName.get(m.errors.alias);
      if (!t.errors || t.errors.kind === "alias") fail(`${m.name}: errors alias chain does not terminate`);
      m.errors = { ...t.errors, resolvedFrom: m.errors.alias };
    }
    m.errorList = m.errors ? (m.errors.list ?? []) : [];
    m.inheritsRecordShape = Boolean(m.result && (m.result.kind === "categoryResult" || m.result.recordShape));
    m.resultSummary =
      m.result?.kind === "categoryResult" ? `the shared result shape declared in "${m.result.inheritedFrom}"`
      : m.result?.recordShape ? `${m.result.recordTypes.join(", ")} records, shape declared in "${m.result.inheritedFrom}"`
      : m.result ? "see the documented shape"
      : null;
  }

  // The four-way drift gate.
  const dispatch = dispatchMethods(root);
  const readme = readmeMethods(root);
  const sdk = sdkMethods(root);

  const sources = [
    { key: "rpc.rs dispatch", names: dispatch.names, source: dispatch.source },
    { key: "RPC_REFERENCE.md headings", names: docNames, source: "docs/RPC_REFERENCE.md" },
    { key: "README method table", names: readme.names, source: readme.source },
    { key: "Python SDK call sites", names: sdk.names, source: sdk.source },
  ];

  const union = new Set(sources.flatMap((s) => [...s.names]));
  const disagreements = [];
  for (const s of sources) {
    const missing = setDiff(union, s.names);
    if (missing.length) disagreements.push({ source: s.key, missing });
  }

  const facts = measureDriftFacts(root, doc, byName);

  // KNOWN_DRIFT, evaluated in both directions.
  const active = [];
  const stale = [];
  for (const entry of KNOWN_DRIFT) {
    if (entry.holds(facts)) active.push(entry);
    else stale.push(entry);
  }

  // Doc constants and network parameters.
  const limitsHeading = lines.findIndex((l) => /^##\s+Limits\s*$/.test(l));
  if (limitsHeading === -1) fail("docs/RPC_REFERENCE.md: the Limits section was not found");
  const limitsEnd = lines.findIndex((l, i) => i > limitsHeading && /^##\s/.test(l));
  const limitsTable = parseTable(lines, inFence, limitsHeading + 1, limitsEnd);
  if (!limitsTable) fail("docs/RPC_REFERENCE.md: the Limits table was not found");

  const errorsHeading = lines.findIndex((l) => /^##\s+Error codes\s*$/.test(l));
  if (errorsHeading === -1) fail("docs/RPC_REFERENCE.md: the Error codes section was not found");
  const errorsEnd = lines.findIndex((l, i) => i > errorsHeading && /^##\s/.test(l));
  const errorTables = [];
  for (let i = errorsHeading + 1; i < errorsEnd; i++) {
    const t = parseTable(lines, inFence, i, errorsEnd);
    if (t) {
      errorTables.push(t);
      i = i + t.rows.length + 2;
    }
  }
  if (errorTables.length < 2) fail("docs/RPC_REFERENCE.md: expected a standard and a server-defined error table");
  const errorCatalogue = errorTables.flatMap((t) =>
    t.rows.map((r) => ({
      code: Number(stripTicks(r.Code)),
      meaning: r.Meaning,
      trigger: r["Where it comes from"] ?? r["Common triggers"] ?? null,
    }))
  );

  const signalsFile = "crates/ai_entities/src/signals.rs";
  const memoryFile = "crates/ai_entities/src/memory.rs";
  const signalTypes = enumVariants(
    enumBody(readFileSync(join(root, signalsFile), "utf8"), "AiSignalType", signalsFile), "AiSignalType", signalsFile);
  const memoryObjectTypes = enumVariants(
    enumBody(readFileSync(join(root, memoryFile), "utf8"), "MemoryObjectType", memoryFile), "MemoryObjectType", memoryFile);

  // The doc's signal-type table carries a payload note for every type, so it
  // covers the seven variants that have no doc comment in source.
  const signalCategory = categoryData.get("Signal methods");
  const signalNotes = new Map(
    (signalCategory?.extras?.["Signal types"] ?? []).map((r) => [Number(r.Byte), { label: stripTicks(r.Variant), note: r.Notes }])
  );
  if (signalNotes.size !== signalTypes.length) {
    fail(`signal types: the doc table lists ${signalNotes.size} and the enum has ${signalTypes.length}`);
  }
  const signals = signalTypes.map((v) => {
    const doc = signalNotes.get(v.discriminant);
    if (!doc) fail(`signal type ${v.discriminant} (${v.variant}) is missing from the doc table`);
    return {
      ...v,
      wireName: doc.label,
      payloadNote: doc.note,
      descriptionSource: v.description ? `doc comment on ${signalsFile}` : null,
    };
  });
  const undocumentedSignals = signals.filter((s) => s.description === null).map((s) => s.variant);

  const payloads = txTypes(root);
  const retentionBlocks = namedConst(root, "crates/consensus/src/lib.rs", "PRUNE_RETAIN_BLOCKS");

  // -------------------------------------------------------------------------
  // Source-derived datasets, and the cross-checks that make them gates
  // -------------------------------------------------------------------------

  // Source links.
  //
  // What actually stops a published line number from going stale is --check in
  // prebuild: rpc.rs moving changes these numbers, the committed JSON stops
  // matching a fresh run, and the build fails until it is regenerated. That is
  // the gate, and it is the reason the link carries a line at all.
  //
  // The assertion below is NOT that gate. It reads back from the same parse
  // that produced the number, so it cannot detect staleness; it is a parser
  // self-check that catches an off-by-one or a mis-sliced match block. Stated
  // plainly because a comment claiming this prevents rot would be false.
  const sourceRefs = [];
  for (const m of built) {
    // A method the dispatch table does not carry is drift, and the drift gate
    // reports it with far more useful detail than a missing-source-link error
    // would. Yielding to it keeps the better diagnostic.
    if (!dispatch.names.has(m.name)) continue;
    const line = dispatch.armLine.get(m.name);
    if (line === undefined) fail(`${m.name}: no dispatch arm line was recorded, so no source link can be generated`);
    const armText = dispatch.emitted.split("\n")[line - 1] ?? "";
    if (!armText.includes(`"${m.name}"`)) {
      fail(
        `${dispatch.source}:${line} no longer carries the dispatch arm for ${m.name}. ` +
        `The source link would still resolve but point at the wrong line, so regenerate.`
      );
    }
    sourceRefs.push({ name: m.name, file: dispatch.source, line });
  }

  const errorCodes = errorCodesFromSource(root);
  const httpRejections = httpRejectionsFromSource(root);
  const sourceLimits = limitsFromSource(root);
  const fees = feesFromSource(root, payloads);
  const bpsFees = bpsFeesFromSource(root);
  const wire = txWireLayout(root);
  const capabilities = capabilityBits(root);
  const quorum = quorumRule(root);
  const coverageMatrix = sdkCoverage(root, payloads, signals.length, memoryObjectTypes.length);

  const signalTails = rustConstsMatching(root, "crates/execution/src/lib.rs", "[A-Z0-9_]+_EXTRA(?:_FIXED)?_LEN");
  const signalBaseLen = rustConst(root, "crates/execution/src/lib.rs", "SIGNAL_COMMITMENT_PAYLOAD_V1_BASE_LEN");
  const entityCaps = rustConstsMatching(root, "crates/ai_entities/src/memory.rs", "MAX_[A-Z0-9_]+");

  const retentionHorizons = {
    disk: { ...rustConst(root, "crates/consensus/src/lib.rs", "PRUNE_RETAIN_BLOCKS"), what: "blocks retained on disk" },
    index: { ...rustConst(root, "crates/node/src/main.rs", "MAX_INDEX_ENTRIES"), what: "heights retained in the in-memory block index" },
  };

  // The two horizons are deliberately different sizes and a client has to know
  // which one it is past. Publishing them as a pair only makes sense while the
  // index reaches further back than the disk does, so that ordering is asserted
  // rather than assumed.
  if (!(retentionHorizons.index.value >= retentionHorizons.disk.value)) {
    fail(
      `retention horizons: the in-memory index (${retentionHorizons.index.value}) no longer reaches at least as ` +
      `far back as the pruned disk (${retentionHorizons.disk.value}), so the documented gap between them is inverted`
    );
  }

  // Cross-check 1: the error codes the node emits against the codes the
  // document lists. KNOWN_DRIFT carries the accepted difference, so a NEW
  // undocumented code fails the build rather than appearing unannounced.
  const docCodes = new Set(errorCatalogue.map((e) => e.code));
  const srcCodes = new Set(errorCodes.map((e) => e.code));
  const exceptedCodes = new Set(KNOWN_DRIFT.flatMap((e) => e.codes ?? []));
  const undocumentedCodes = setDiff(srcCodes, docCodes).filter((c) => !exceptedCodes.has(c));
  const phantomCodes = setDiff(docCodes, srcCodes);
  if (undocumentedCodes.length || phantomCodes.length) {
    fail(
      `error codes: the implementation and docs/RPC_REFERENCE.md disagree. ` +
      `Emitted but undocumented and not in KNOWN_DRIFT: ${undocumentedCodes.join(", ") || "none"}. ` +
      `Documented but not emitted: ${phantomCodes.join(", ") || "none"}.`
    );
  }

  // Cross-check 2: the limits table against the constants it cites. The doc
  // names the constant in its Source column, so the two can be joined and the
  // values compared rather than trusted.
  // The document writes values in human units ("512 KiB", "10 MiB") and its
  // Source column may carry a multiplier ("MAX_TX_SIZE x 2", because the hex
  // encoding is twice the binary size). Converting the constant INTO the
  // document's own unit and multiplier makes this an exact comparison rather
  // than a guess at which of several plausible renderings was meant.
  const UNIT_DIVISOR = new Map([["KiB", 1024], ["MiB", 1024 * 1024], ["GiB", 1024 * 1024 * 1024]]);
  const limitByConst = new Map(sourceLimits.map((l) => [l.name, l]));
  let limitsCompared = 0;
  for (const row of limitsTable.rows) {
    const cited = [...String(row.Source).matchAll(/`?\b([A-Z][A-Z0-9_]{3,})\b/g)].map((m) => m[1]);
    for (const name of cited) {
      if (!limitByConst.has(name)) continue;
      const constant = limitByConst.get(name);
      const valueText = String(row.Value);
      const lead = /([0-9][0-9\s,]*)/.exec(valueText);
      if (!lead) continue;
      const stated = Number(lead[1].replace(/[\s,]/g, ""));
      const unit = [...UNIT_DIVISOR.keys()].find((u) => new RegExp(`^[0-9\\s,]*${u}\\b`).test(valueText));
      const multiplier = /[x×*]\s*(\d+)/.exec(String(row.Source));
      const scaled =
        (constant.value / (unit ? UNIT_DIVISOR.get(unit) : 1)) * (multiplier ? Number(multiplier[1]) : 1);
      limitsCompared += 1;
      if (stated !== scaled) {
        fail(
          `limits: the document says ${name} is "${valueText}" which is ${stated}${unit ? " " + unit : ""}, ` +
          `but the constant is ${constant.value} at ${constant.file}:${constant.line}` +
          `${multiplier ? ` with the document's x${multiplier[1]} applied` : ""}, which is ${scaled}. ` +
          `One of the two is stale.`
        );
      }
    }
  }
  // A cross-check that compared nothing is not a cross-check. If the Source
  // column stops naming constants, this fails rather than passing vacuously.
  if (limitsCompared !== sourceLimits.length) {
    fail(
      `limits: expected to compare all ${sourceLimits.length} constants against the document's table but ` +
      `matched ${limitsCompared}. The table's Source column no longer names every constant.`
    );
  }

  // The Observed gaps table, which is the document's own account of what has
  // shipped at the protocol layer but is not reachable over RPC.
  const gapsHeading = lines.findIndex((l) => /^##\s+Observed gaps\s*$/.test(l));
  if (gapsHeading === -1) fail("docs/RPC_REFERENCE.md: the Observed gaps section was not found");
  const gapsEndRaw = lines.findIndex((l, i) => i > gapsHeading && /^##\s/.test(l));
  const gapsEnd = gapsEndRaw === -1 ? lines.length : gapsEndRaw;
  // parseTable stops at the first prose line, and this section opens with a
  // paragraph, so advance to the table itself before parsing.
  let gapsStart = -1;
  for (let i = gapsHeading + 1; i < gapsEnd; i++) {
    if (!inFence[i] && lines[i].trim().startsWith("|")) { gapsStart = i; break; }
  }
  if (gapsStart === -1) fail("docs/RPC_REFERENCE.md: no table found under ## Observed gaps");
  const gapsTable = parseTable(lines, inFence, gapsStart, gapsEnd);
  if (!gapsTable) fail("docs/RPC_REFERENCE.md: the Observed gaps table was not found");

  const coverage = {
    methods: built.length,
    withCurl: built.filter((m) => m.curl).length,
    withOwnParamsTable: built.filter((m) => m.params?.kind === "table" && !m.params.resolvedFrom).length,
    withParamsResolved: built.filter((m) => m.params !== null).length,
    withOwnResultFence: built.filter((m) => m.result?.kind === "fence" && !m.result.resolvedFrom).length,
    withResultResolved: built.filter((m) => m.result !== null).length,
    withOwnErrors: built.filter((m) => m.errors && !m.errors.resolvedFrom && m.errors.kind !== "categoryCommon").length,
    withErrorsResolved: built.filter((m) => m.errors !== null).length,
    withSampleResponse: built.filter((m) => m.sampleResponse).length,
    resolvedByAlias: built.filter((m) => m.params?.resolvedFrom || m.result?.resolvedFrom || m.errors?.resolvedFrom).length,
    inheritingRecordShape: built.filter((m) => m.inheritsRecordShape).length,
    inheritingWholeResult: built.filter((m) => m.result?.kind === "categoryResult").length,
    inheritingCommonErrors: built.filter((m) => m.errors?.kind === "categoryCommon").length,
  };

  return {
    methods: built,
    drift: { sources: sources.map((s) => ({ key: s.key, source: s.source, count: s.names.size })), union: [...union].sort(), disagreements, active, stale, facts },
    errorCatalogue,
    limits: limitsTable.rows,
    signals,
    undocumentedSignals,
    memoryObjectTypes,
    txTypes: payloads,
    retentionBlocks,
    coverage,
    docNames,
    sourceRefs,
    errorCodes,
    httpRejections,
    sourceLimits,
    fees,
    bpsFees,
    wire,
    capabilities,
    quorum,
    coverageMatrix,
    signalTails,
    signalBaseLen,
    entityCaps,
    retentionHorizons,
    observedGaps: gapsTable.rows,
  };
}

function payloadFor(c) {
  return {
    source: {
      reference: "docs/RPC_REFERENCE.md",
      dispatch: "crates/node/src/rpc.rs",
      methodTable: "README.md",
      sdk: "sdk/novai-python-sdk/novai_sdk",
      note: "Every value on this page is read from these files at build time. None is typed by hand.",
    },
    coverage: {
      value: c.coverage,
      method:
        "counted over the parsed document: own means declared in the method's own section, resolved means " +
        "available after alias resolution and category-shape inheritance",
    },
    drift: {
      value: {
        agreedMethodCount: c.docNames.size,
        sources: c.drift.sources,
        disagreements: c.drift.disagreements,
        knownExceptions: c.drift.active.map((e) => ({
          id: e.id,
          summary: e.summary,
          why: e.why,
          operatorRef: e.operatorRef,
        })),
      },
      method: "four-way name-set equality across the dispatch table, the reference headings, the README table and the SDK call sites",
    },
    methods: {
      value: c.methods.map((m) => ({
        name: m.name,
        category: m.category,
        brief: m.brief,
        description: m.description,
        params: m.params,
        result: m.result,
        errors: m.errors,
        curl: m.curl,
        sampleResponse: m.sampleResponse,
        exampleNote: m.exampleNote,
      })),
      method: "parsed from the labelled blocks under each ### heading in docs/RPC_REFERENCE.md",
    },
    errorCatalogue: { value: c.errorCatalogue, method: "the two tables under ## Error codes in docs/RPC_REFERENCE.md" },
    limits: { value: c.limits, method: "the table under ## Limits in docs/RPC_REFERENCE.md" },
    networkParameters: {
      value: { blockRetention: { blocks: c.retentionBlocks, constant: "PRUNE_RETAIN_BLOCKS" } },
      method: "named constants read from crates/consensus/src/lib.rs; the wall-clock equivalent is deliberately not derived because cadence moves",
    },
    signalTypes: {
      value: c.signals,
      method: "unit variants of enum AiSignalType joined to the doc's signal-type table by discriminant; description is the source doc comment where one exists",
    },
    memoryObjectTypes: { value: c.memoryObjectTypes, method: "unit variants of enum MemoryObjectType in crates/ai_entities/src/memory.rs" },
    txTypes: { value: c.txTypes, method: "pub const *_PAYLOAD_V1: u8 discriminants in crates/execution/src/lib.rs, contiguity asserted" },
    sourceRefs: {
      value: c.sourceRefs,
      method:
        "the line of each method's dispatch arm in crates/node/src/rpc.rs; every recorded line is re-read and " +
        "must still carry its own method name, so a link cannot go on pointing at the wrong line",
    },
    errorCodes: {
      value: c.errorCodes,
      method:
        "every -32xxx literal emitted by crates/node/src/rpc.rs, across all three emission forms (RpcError literal, " +
        "mempool tuple arm, and the hand-built JSON of the too-large path), cross-checked against the document's " +
        "error tables; a code emitted but undocumented fails the build unless carried in KNOWN_DRIFT",
    },
    httpRejections: {
      value: c.httpRejections,
      method:
        "StatusCode(N) sites in crates/node/src/rpc.rs with their body literal. These carry no JSON-RPC envelope, " +
        "so a client that calls .json() on them raises a parse error rather than reading an error object",
    },
    sourceLimits: {
      value: c.sourceLimits,
      method: "the named constants themselves, joined to the document's Limits table by the constant it cites and compared",
    },
    fees: {
      value: c.fees,
      method:
        "the match arms of minimum_fee_for_tx in crates/execution/src/lib.rs, joined to the payload discriminants. " +
        "Every one of the transaction types is covered or the build fails",
    },
    bpsFees: {
      value: c.bpsFees,
      method: "basis-point fee constants in crates/execution/src/lib.rs against BPS_DENOMINATOR",
    },
    txWireLayout: {
      value: c.wire,
      method:
        "the write sequence of encode_tx_v1_unsigned in crates/codec/src/lib.rs, in order, with each field's width " +
        "and endianness taken from the writer it uses",
    },
    capabilityBits: {
      value: c.capabilities,
      method: "the bit tests in Capabilities::from_byte in crates/ai_entities/src/lib.rs",
    },
    quorum: {
      value: c.quorum,
      method:
        "the body of the quorum const fn, read from both sites that implement it and asserted identical; the " +
        "validator count is a configuration fact and is deliberately NOT generated",
    },
    signalPayloads: {
      value: { baseLength: c.signalBaseLen, tails: c.signalTails },
      method: "the signal commitment base length and every *_EXTRA_LEN constant in crates/execution/src/lib.rs",
    },
    entityCaps: {
      value: c.entityCaps,
      method: "pub const MAX_* declarations in crates/ai_entities/src/memory.rs with their doc comments",
    },
    retentionHorizons: {
      value: c.retentionHorizons,
      method:
        "PRUNE_RETAIN_BLOCKS in crates/consensus/src/lib.rs and MAX_INDEX_ENTRIES in crates/node/src/main.rs. " +
        "Published in blocks only: the wall-clock equivalent moves with cadence and is left for the reader to derive " +
        "from the live rate",
    },
    sdkCoverage: {
      value: c.coverageMatrix,
      method:
        "transaction builders exported by each SDK matched to the payload discriminants on a canonical name key, " +
        "plus the signal and memory type counts each SDK declares. The Rust SDK re-exports the chain's own enums, " +
        "which is asserted, so its type coverage is structural rather than maintained",
    },
    observedGaps: {
      value: c.observedGaps,
      method: "the table under ## Observed gaps in docs/RPC_REFERENCE.md",
    },
    gaps: {
      value: {
        signalTypesWithoutSourceDescription: c.undocumentedSignals,
        note:
          "These variants carry no doc comment in the Rust source, so no description can be generated for them. " +
          "The payload note from the reference table is present for all types; only the prose description is missing.",
      },
      method: "variants of AiSignalType with no preceding /// comment",
    },
  };
}

function stableRender(obj) {
  return JSON.stringify(obj, null, 2) + "\n";
}

function main() {
  const args = parseArgs(process.argv);
  const c = compute(args.root);

  // Drift gate, both directions.
  let driftFailed = false;
  if (c.drift.disagreements.length > 0) {
    console.error("console-data: drift gate: the four method-name sources disagree.");
    for (const d of c.drift.disagreements) {
      console.error(`  ${d.source} is missing: ${d.missing.join(", ")}`);
    }
    driftFailed = true;
  }
  if (c.drift.stale.length > 0) {
    console.error("console-data: drift gate: a KNOWN_DRIFT exception no longer applies. Delete it.");
    for (const e of c.drift.stale) {
      console.error(`  ${e.id}: no longer present. Remove it from KNOWN_DRIFT and close ${e.operatorRef}.`);
    }
    driftFailed = true;
  }

  // Printed on EVERY run, not only on failure.
  console.log(`console-data: KNOWN_DRIFT: ${c.drift.active.length} accepted exception(s), each open in NEEDS-OPERATOR.md`);
  for (const e of c.drift.active) {
    console.log(`  - ${e.id} (${e.operatorRef}): ${e.summary}`);
  }
  if (driftFailed) fail("drift gate did not pass");

  // Grouped by file and code point, with the first location kept as the proof.
  // Reading crates/ brought in source whose COMMENTS carry em dashes, which
  // printed one line each and buried the KNOWN_DRIFT block above. The evidence
  // that a substitution happened is what matters, not one line per occurrence.
  const subsByKey = new Map();
  for (const s of substitutionLog) {
    const key = `${s.source}::${s.from}`;
    if (!subsByKey.has(key)) subsByKey.set(key, { ...s, count: 0 });
    subsByKey.get(key).count += 1;
  }
  for (const s of [...subsByKey.values()].sort((a, b) => a.source.localeCompare(b.source))) {
    const where = `${s.source}:${s.line}:${s.column}`;
    console.log(
      s.count === 1
        ? `console-data: normalised ${s.from} to "${s.to}" at ${where}`
        : `console-data: normalised ${s.count} x ${s.from} to "${s.to}" in ${s.source} (first at ${where})`
    );
  }
  if (substitutionLog.length === 0) {
    console.log("console-data: no forbidden dash code points found in the sources");
  }

  const payload = payloadFor(c);
  const openrpcBase = buildOpenRpc(c.methods, "Examples in the reference assume a local endpoint. Set URL to the endpoint you are querying.");

  // Determinism: reuse the previous timestamp when neither payload changed, so
  // an unchanged tree produces byte-identical output and --check is a byte
  // compare. Both files share one timestamp because they share one parse.
  const prevData = readJson(args.out);
  const prevRpc = readJson(args.openrpc);
  const dataUnchanged = prevData !== null && stableRender(stripKey(prevData, "generatedAt")) === stableRender(payload);
  const rpcUnchanged = prevRpc !== null && stableRender(stripInfoStamp(prevRpc)) === stableRender(openrpcBase);
  const previousStamp = typeof prevData?.generatedAt === "string" ? prevData.generatedAt : null;
  const generatedAt = dataUnchanged && rpcUnchanged && previousStamp ? previousStamp : new Date().toISOString();

  const dataOut = stableRender({ generatedAt, ...payload });
  const rpcOut = stableRender({ ...openrpcBase, info: { ...openrpcBase.info, "x-generatedAt": generatedAt } });

  if (args.check) {
    checkFile(args.out, dataOut);
    checkFile(args.openrpc, rpcOut);
    console.log("console-data: check ok (both committed files match a fresh run)");
    return;
  }

  writeAtomic(args.out, dataOut);
  writeAtomic(args.openrpc, rpcOut);
  console.log(`console-data: wrote ${args.out}`);
  console.log(`console-data: wrote ${args.openrpc}`);
  const cv = c.coverage;
  console.log(`  methods: ${cv.methods}  curl: ${cv.withCurl}  params own/resolved: ${cv.withOwnParamsTable}/${cv.withParamsResolved}  result own/resolved: ${cv.withOwnResultFence}/${cv.withResultResolved}  errors own/resolved: ${cv.withOwnErrors}/${cv.withErrorsResolved}`);
  console.log(`  resolved by alias: ${cv.resolvedByAlias}  inheriting a record shape: ${cv.inheritingRecordShape} (of which whole-result: ${cv.inheritingWholeResult})  inheriting common errors: ${cv.inheritingCommonErrors}  sample responses: ${cv.withSampleResponse}`);
  console.log(`  signal types with no source description: ${c.undocumentedSignals.length}${c.undocumentedSignals.length ? ` (${c.undocumentedSignals.join(", ")})` : ""}`);
}

function readJson(path) {
  if (!existsSync(path)) return null;
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
}

function stripKey(obj, key) {
  const { [key]: _drop, ...rest } = obj;
  return rest;
}

function stripInfoStamp(doc) {
  if (!doc || typeof doc !== "object" || !doc.info) return doc;
  return { ...doc, info: stripKey(doc.info, "x-generatedAt") };
}

function checkFile(path, expected) {
  if (!existsSync(path)) fail(`--check: ${path} does not exist; run npm run console:data and commit it`);
  if (readFileSync(path, "utf8") !== expected) {
    fail(`--check: ${path} is stale relative to the repo; run npm run console:data and commit the result`);
  }
}

function writeAtomic(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  const tmp = path + ".tmp";
  writeFileSync(tmp, content);
  renameSync(tmp, path);
}

main();
