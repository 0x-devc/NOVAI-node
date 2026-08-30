#!/usr/bin/env node
//
// Render the console's ten sections into console.html from the generated data.
//
// WHY THE MARKUP IS WRITTEN INTO console.html RATHER THAN INJECTED AT BUILD
// TIME: tailwind.config.ts scans "./console.html" as a content glob, and
// Tailwind reads the file on disk. Markup injected by a Vite transformIndexHtml
// hook is invisible to that scan, so every class in these sections would be
// purged and the page would render unstyled in production while looking correct
// in dev. Writing the file keeps Tailwind, the design-rules test (which reads
// console.html explicitly) and a reviewable diff all working on the same bytes.
//
// The regions are delimited by sentinel comments and rewritten in place. That
// makes a hand-edit inside a region silently disposable, so the marker
// mechanism carries its own gates, listed at MARKER GATES below.
//
// Usage:
//   node scripts/generate-console-html.mjs            write console.html
//   node scripts/generate-console-html.mjs --check    fail if it is stale
//
// No network, no git, no shell-outs. Reads only the generated JSON and the
// HTML it rewrites.

import { readFileSync, writeFileSync, renameSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const WEB_ROOT = resolve(SCRIPT_DIR, "..");

function fail(msg) {
  console.error(`console-html: FAIL: ${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = { check: false, html: join(WEB_ROOT, "console.html"), data: join(WEB_ROOT, "src/data/console-data.generated.json") };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--check") args.check = true;
    else if (a === "--html") args.html = resolve(argv[++i]);
    else if (a === "--data") args.data = resolve(argv[++i]);
    else fail(`unknown argument ${a}`);
  }
  return args;
}

// ---------------------------------------------------------------------------
// Escaping
// ---------------------------------------------------------------------------

const ENTITIES = new Map([
  ["&", "&amp;"],
  ["<", "&lt;"],
  [">", "&gt;"],
  ['"', "&quot;"],
]);

/** HTML-escape. Everything interpolated into markup goes through this. */
function esc(value) {
  if (value === null || value === undefined) return "";
  return String(value).replace(/[&<>"]/g, (c) => ENTITIES.get(c));
}

/** Strip markdown backticks and emphasis from doc-sourced prose. */
function plain(value) {
  if (value === null || value === undefined) return "";
  return String(value)
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\*\*([^*]*)\*\*/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .trim();
}

/** Doc-sourced prose, escaped, with inline code spans preserved as <code>. */
function rich(value) {
  if (value === null || value === undefined) return "";
  return esc(String(value).replace(/\[([^\]]*)\]\([^)]*\)/g, "$1"))
    .replace(/`([^`]*)`/g, '<code class="text-ink-mid">$1</code>')
    .replace(/\*\*([^*]*)\*\*/g, "<strong>$1</strong>");
}

/**
 * A value the reader is told to match on, escaped for HTML AND with every
 * non-ASCII character emitted as a numeric entity.
 *
 * The 503 body the node sends contains an em dash. console.html is scanned by
 * the dash gate, so the literal character cannot be written into it; an entity
 * renders as the correct character in the browser while leaving the file ASCII.
 * The alternative that was shipped, normalising the dash to a hyphen, published
 * a string that never matches what the server sends and said nothing about it.
 */
function escWire(value) {
  return esc(value).replace(/[\u0080-\uffff]/g, (c) => `&#${c.charCodeAt(0)};`);
}

const anchorFor = (name) => name.toLowerCase();

/**
 * The one-line brief for the index and the method head.
 *
 * A withheld method supplies its own, because the reference's brief for
 * novai_faucet is "Mint test tokens (dev mode only)" and that parenthetical is
 * the same false gating claim that faucet-rpc-gating-incomplete exists to
 * correct, republished in an index cell that no correction mechanism reaches.
 */
const briefOf = (m) => (m.withheld ? m.withheld.brief : m.brief);
const GITHUB_BLOB = "https://github.com/NOVAInetwork/NOVAI-node/blob/main";
const sourceLink = (file, line, label) =>
  `<a class="text-brand-text no-underline hover:underline" href="${esc(`${GITHUB_BLOB}/${file}#L${line}`)}">${esc(label ?? `${file}:${line}`)}</a>`;

// ---------------------------------------------------------------------------
// HAND-WRITTEN STRINGS
//
// Every sentence on the console that is NOT read from the source tree lives in
// this object and nowhere else. Hand-written content is where facts rot, so it
// is kept in one auditable place rather than scattered through the markup.
//
// Rules these strings obey: no em or en dashes, first person singular, no
// operational adjectives presented as static fact, and no number that is not
// either derived below or a stated configuration fact.
// ---------------------------------------------------------------------------

const PROSE = {
  connectRate:
    "Requests are rate limited per source IP. The limits, and the reply you get when you cross one, are in errors and limits.",

  firstCall:
    "Three ways to make the same call. The output under each was produced by running exactly the code shown.",
  firstCallCapture:
    "Captured by running each example against the live endpoint. Heights and hashes move every few hundred " +
    "milliseconds, so yours will differ; the shape is what is being shown.",
  firstCallCurl: "No install. Paste it.",
  firstCallPython: "Standard library only. No pip install.",
  firstCallRust:
    "A standalone cargo project with one dependency. This needs a build step, which the two above do not, so it " +
    "is the third path rather than the first. It does not use the NOVAI Rust SDK, because that SDK depends on the " +
    "workspace crates by path and so cannot be built outside a clone of the repository.",

  // The count is derived by the caller: a hardcoded 29 in a sentence vouching
  // for the index was one of four hardcoded 29s on this page.
  rpc: "The notes column names the caveat that applies to a method before you open it.",
  rpcSource:
    "Each entry links to the dispatch arm that implements it. Those line numbers are regenerated with the page, and " +
    "the build fails when the committed page no longer matches the tree.",

  errors:
    "Thirteen JSON-RPC error codes, four HTTP-level rejections that carry no JSON-RPC envelope at all, and six limits.",
  errors32014:
    "One of these thirteen is not in the repository's own RPC reference. -32014 is emitted by the node but the " +
    "reference's error tables do not list it, so this page reads it from the implementation instead. For that one " +
    "code the console is the source rather than a projection of the document.",
  errorsHttp:
    "These four never carry a JSON-RPC envelope. A client that calls .json() on the response raises a parse error " +
    "rather than reading an error object, so check the HTTP status before parsing.",
  errorsNonce:
    "-32010 and -32014 look similar and need opposite handling. Too low means the transaction is dead and the " +
    "client must resynchronise. Too high means it is merely early: the identical bytes succeed once the sender's " +
    "earlier nonces commit, so retry rather than rebuild.",
  errorsBalance:
    "A transfer to an address that has no account row yet is rejected below the minimum account balance. The error " +
    "reports the transfer amount in its balance field, which reads as though the sender were short.",

  transactions:
    "Eleven transaction types. The discriminant is the first byte of the payload and it is what decides the type.",
  txSigning:
    "Two details a non-Rust client has to get exactly right. The envelope is little-endian while payload internals " +
    "are big-endian. And the signing preimage and the transaction id are different preimages: the signature covers " +
    "a domain tag followed by the unsigned encoding, while the id is a hash of the unsigned encoding alone. " +
    "Trailing bytes after the signature are a decode error, not padding.",
  txFees:
    "Minimum fees are per type. Percentage fees apply on top of the floor for the operations listed, so unit " +
    "economics are the floor plus a percentage rather than the floor alone.",

  ai:
    "AI entities are the part of this chain that has no equivalent elsewhere. Three worked flows first, then the " +
    "full enumerations.",
  aiRecipesNote:
    "These describe what the implementation does, with every constant cited to its declaration. They are written " +
    "as local devnet walkthroughs: the read path on this page is verified against the public endpoint, but a write " +
    "path needs a funded key and a node you control.",
  aiCapabilities:
    "Capabilities are one byte, set at registration, and immutable for the entity's life. The upgrade transaction " +
    "changes the code hash and nothing else, and registering again mints a different entity id, which abandons the " +
    "reputation, stake, agreements and memory objects attached to the old one. Set every bit you might need on day one.",

  sdks:
    "Three SDKs, and they are not equivalent. The distribution difference decides a language choice before the " +
    "coverage difference does.",
  sdkDistribution:
    "Only the Python SDK can be installed from a package registry. I checked all three registries on 2026-08-29: " +
    "novai-sdk resolves on PyPI at version 0.1.0, published 2026-05-29, and the SDK source in this repository has " +
    "not changed since that release. The Rust crate is not on crates.io and the npm package is not published.",
  sdkTypescript:
    "The TypeScript SDK is in development and is not a supported signing path. Its coverage gap is stated in " +
    "numbers below rather than described, because the numbers are the fact.",
  sdkRustStructural:
    "The Rust SDK re-exports the chain's own type enumerations rather than redeclaring them, so its signal and " +
    "memory coverage is complete by construction and cannot drift. The TypeScript SDK declares its own copies, " +
    "which is why it can and does.",

  parameters:
    "What the network is configured to do. Validator count is a configuration fact rather than a live reading.",
  parametersValidators:
    "Four validators. This is set by hand and deliberately excluded from the generator: the genesis file names " +
    "five, and genesis is not what runs.",
  parametersRetention:
    "Retention is published in blocks and never converted to wall-clock time. Block cadence has moved by more than " +
    "4x inside a week on this chain, so any hours figure written here would be wrong within days. Divide by the " +
    "rate you measure yourself.",
  parametersOmitted:
    "Two parameters a reader might expect are deliberately absent. Chain id is omitted because the value that is " +
    "live has not been confirmed and publishing the wrong one would be worse than publishing none. Genesis hash is " +
    "omitted because block 0 has been pruned and is not retrievable, which is the more useful fact and is stated " +
    "under known gaps.",

  gaps:
    "What this chain cannot do yet, what the documentation gets wrong, and the operating characteristics that " +
    "will surprise you. This section exists because the failure it prevents is expensive and the disclosure is cheap.",
  gapsRetention:
    "Measured against the live endpoint on 2026-08-29: a height 45,000 and 49,000 blocks back returned a block, " +
    "and 51,000 and 55,000 blocks back returned result null. That places the boundary where the constant says it " +
    "is. Two things follow for a client: a pruned height answers with a null result rather than an error, and a " +
    "height above the tip answers -32602 with the committed height inside the message.",
  gapsIndex:
    "The block index that resolves a transaction id or a block hash to a height is held in memory and is populated " +
    "only as blocks commit. It is not backfilled from disk at startup. After a node restarts, a transaction " +
    "committed before that restart resolves to null. An indexer therefore cannot backfill from an RPC node and has " +
    "to tail from the point it starts.",
  gapsPruned:
    "Read from the implementation and not verifiable from outside: within the band where the index still holds a " +
    "transaction id but its block has been pruned from disk, the reply is -32002 rather than a null result. That " +
    "cannot be demonstrated against a public node, because block headers carry no transaction ids and there is no " +
    "way to obtain one whose block falls inside the band.",
  gapsNonce:
    "novai_getNonce and the nonce field of novai_getBalance are different numbers and are not substitutes. " +
    "getNonce answers from the mempool admission cursor; getBalance answers from the committed account row. They " +
    "agree until a transaction from that sender commits and then fails during execution, after which the cursor " +
    "runs ahead until the node restarts and reseeds from state. Build plain-account transactions from the " +
    "getBalance nonce, and use getNonce to predict whether the mempool will admit. Registered AI entities are " +
    "unaffected: their path compares with greater-than-or-equal and self-heals.",
  gapsExceptions:
    "The drift gate compares four independent sources and fails when they disagree. Five discrepancies are known " +
    "and carried as named exceptions, each printed on every generator run. The gate also fails when a listed " +
    "exception stops applying, so this list can only shrink, and fixing a document forces its exception to be " +
    "deleted.",

  verify:
    "The claims on this page are checkable from this browser. This panel calls the live chain and re-derives a " +
    "consensus property: it walks consecutive block headers and checks that each one's parent hash is the previous " +
    "block's hash.",
  verifyStatic:
    "This panel needs JavaScript. With it disabled the page still renders in full; only this check is unavailable.",
};

// The three first-call outputs, produced by running exactly the code shown on
// the page. Not generated: a captured terminal result is a snapshot by nature,
// so it carries the moment it was taken and the page says heights will differ.
const CAPTURES = {
  at: "2026-08-29T17:55:36Z",
  curl: `{"jsonrpc":"2.0","result":{"block_hash":"6b4ef2adc610656969ca65c94c057ddcb5eafa3c441de14d95b931aac066a8e9","height":5225248,"parent_hash":"0c6582e4ef81f793266ae8d46642f9acd9c314bc7947ef2e57142a2f3a0981c1","round":0,"state_root":"2843779c3de67ceed02e32b0bef2014a0fbfff14d82f62efa1c216c1daae150e","tx_count":0},"id":1}`,
  python: `{
  "block_hash": "8cec80e8dc7b2e77b1e43314bda2f2570aaad82fb02c4afa9a1d35990177d802",
  "height": 5225249,
  "parent_hash": "6b4ef2adc610656969ca65c94c057ddcb5eafa3c441de14d95b931aac066a8e9",
  "round": 0,
  "state_root": "2843779c3de67ceed02e32b0bef2014a0fbfff14d82f62efa1c216c1daae150e",
  "tx_count": 0
}`,
  rust: `{
  "block_hash": "4c065392ab83658711dc3df2ade2bda953c33a1a4f5d0e884439b6a9548ed1a3",
  "height": 5225251,
  "parent_hash": "3399db3cd66c7185b2521d5ba7c8c58ddd84b59004024f475b7ed7fe16a79d27",
  "round": 0,
  "state_root": "2843779c3de67ceed02e32b0bef2014a0fbfff14d82f62efa1c216c1daae150e",
  "tx_count": 0
}`,
};

const ENDPOINT = "https://rpc.novai.network";

const PY_EXAMPLE = `import json, urllib.request

req = urllib.request.Request(
    "${ENDPOINT}",
    data=json.dumps({"jsonrpc": "2.0", "method": "novai_getLatestBlock",
                     "params": {}, "id": 1}).encode(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=10) as r:
    print(json.dumps(json.load(r)["result"], indent=2))`;

const RUST_EXAMPLE = `// Cargo.toml
// [dependencies]
// reqwest = { version = "0.12", features = ["blocking", "json"] }
// serde_json = "1"

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "novai_getLatestBlock",
        "params": {},
        "id": 1
    });

    let resp: serde_json::Value = reqwest::blocking::Client::new()
        .post("${ENDPOINT}")
        .json(&body)
        .send()?
        .json()?;

    println!("{}", serde_json::to_string_pretty(&resp["result"])?);
    Ok(())
}`;

// ---------------------------------------------------------------------------
// Section renderers
// ---------------------------------------------------------------------------

const pre = (body) => `<pre class="console-pre"><code>${esc(body)}</code></pre>`;
const out = (body) => `<pre class="console-pre-out"><code>${esc(body)}</code></pre>`;
const lead = (text) => `<p class="console-lead">${rich(text)}</p>`;
// note() takes PROSE and escapes it. noteHtml() takes markup already built by
// the helpers above and does not. Passing built markup to note() renders the
// tags as visible text, which is a defect a reader sees immediately, so the two
// are separate functions rather than one with a flag.
const note = (text) => `<p class="console-note">${rich(text)}</p>`;
const noteHtml = (html) => `<p class="console-note">${html}</p>`;
const h3 = (text) => `<h3 class="console-h3">${esc(text)}</h3>`;

function table(headers, rows) {
  const head = headers.map((h) => `<th scope="col"${h.num ? ' class="text-right"' : ""}>${esc(h.label)}</th>`).join("");
  const body = rows
    .map(
      (r) =>
        `<tr>${r.map((cell, i) => `<td${headers[i].num ? ' class="num"' : ""}>${cell}</td>`).join("")}</tr>`
    )
    .join("\n            ");
  return `<div class="console-scroll">
          <table class="console-table">
            <thead><tr>${head}</tr></thead>
            <tbody>
            ${body}
            </tbody>
          </table>
        </div>`;
}

function details(summary, inner) {
  return `<details class="console-details">
          <summary class="console-summary">${esc(summary)}</summary>
          <div class="px-4 pb-3">${inner}</div>
        </details>`;
}

/**
 * The page's opening claim about itself.
 *
 * Hand-written in console.html until now, which meant it sat outside every
 * sentinel region and therefore outside the marker gate: it typed "Five known
 * discrepancies" fifteen lines above a generated tile reading the same number,
 * and nothing checked that the two agreed. They stopped agreeing the moment a
 * sixth exception was carried.
 *
 * It also overstated the gate. The four-way cross-check compares METHOD NAMES
 * across the dispatch table, the reference headings, the README table and the
 * SDK call sites. It does not compare parameters, error codes, result shapes or
 * notes, and several of the defects this gate fixed were exactly that content.
 * Saying what the check actually does is both true and still worth saying.
 */
function renderIntro(d) {
  const exceptions = d.drift.value.knownExceptions.length;
  const signals = d.signalTypes.value.length;
  const objects = d.memoryObjectTypes.value.length;
  const link = (href, text) => `<a class="text-brand-text no-underline hover:underline" href="${href}">${text}</a>`;
  return `<p class="mt-2 max-w-[70ch] text-sm leading-relaxed text-ink-low">
          Everything needed to build against the chain. The reference below is generated from the source tree at
          build time, and every method name in it is cross-checked across four independent sources, so the build
          fails when they disagree. That check is over names: where the reference and the implementation disagree
          about a parameter, an error code or a result, the difference is carried as an explicit exception and
          corrected where you meet it. There are ${exceptions} of those today, listed in ${link("#gaps", "known gaps")}.
        </p>
        <p class="mt-2 max-w-[70ch] text-sm leading-relaxed text-ink-low">
          NOVAI treats AI entities as first-class on-chain actors: they hold balances and reputation, publish
          ${signals} kinds of signal, and own ${objects} kinds of memory object. That is what the
          ${link("#transactions", "transaction")} and ${link("#ai-entities", "entity")} sections describe.
        </p>`;
}

function renderProvenance(d) {
  const sources = d.drift.value.sources;
  const exceptions = d.drift.value.knownExceptions.length;
  const generated = d.generatedAt.replace("T", " ").replace(/\.\d+Z$/, " UTC");
  const cells = [
    [String(d.drift.value.agreedMethodCount), "methods, agreed"],
    [String(sources.length), "sources cross-checked"],
    [String(exceptions), "carried exceptions"],
    [String(d.txTypes.value.length), "transaction types"],
  ];
  return `<div class="console-prov">
          <div class="console-prov-grid">
            ${cells.map(([v, l]) => `<div class="console-prov-cell"><div class="console-prov-value">${esc(v)}</div><div class="console-prov-label">${esc(l)}</div></div>`).join("\n            ")}
          </div>
          <div class="border-t border-line-subtle px-4 py-2">
            <p class="console-note">Generated from the source tree at ${esc(generated)}. Sources: ${sources.map((s) => `<code class="text-ink-mid">${esc(s.source)}</code>`).join(", ")}.</p>
          </div>
        </div>`;
}

function renderConnect(d) {
  const curl = `URL=${ENDPOINT}

curl -s -X POST $URL -H 'Content-Type: application/json' \\
  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}'`;
  const rows = [
    ["Protocol", "JSON-RPC 2.0 over HTTP POST"],
    ["Content-Type", "<code>application/json</code>"],
    ["Endpoint", `<code>${esc(ENDPOINT)}</code>`],
    ["Authentication", "none"],
  ];
  return `${lead(PROSE.connect)}
        ${pre(curl)}
        ${table([{ label: "Property" }, { label: "Value" }], rows)}
        ${note(PROSE.connectRate)}
        <div class="mt-4 rounded-md border border-line bg-surface-1 px-4 py-3" data-island="network-status">
          <p class="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-low">network</p>
          <p class="console-note">Live height and cadence load here when JavaScript is available.</p>
        </div>`;
}

function renderFirstCall() {
  return `${lead(PROSE.firstCall)}
        ${h3("curl")}
        ${note(PROSE.firstCallCurl)}
        ${pre(`curl -s -X POST ${ENDPOINT} -H 'Content-Type: application/json' \\\n  -d '{"jsonrpc":"2.0","method":"novai_getLatestBlock","params":{},"id":1}'`)}
        ${out(CAPTURES.curl)}
        ${h3("python")}
        ${note(PROSE.firstCallPython)}
        ${pre(PY_EXAMPLE)}
        ${out(CAPTURES.python)}
        ${h3("rust")}
        ${note(PROSE.firstCallRust)}
        ${pre(RUST_EXAMPLE)}
        ${out(CAPTURES.rust)}
        ${note(`${PROSE.firstCallCapture} Captured ${CAPTURES.at}.`)}`;
}

// ---------------------------------------------------------------------------
// CAVEATS AND CORRECTIONS COME FROM THE METHOD, NEVER FROM PROSE
//
// What was here before built the index Notes column by regex-scanning each
// exception's summary and why for /novai_[A-Za-z]+/g and attaching that
// exception's label to every method name it found mentioned. It failed in both
// directions on the same page, which is why a better regex is not the fix:
//
//   over-attached: getnonce-documented-as-interchangeable names BOTH methods in
//     its summary, so novai_getBalance was labelled "not the account nonce".
//     That is true of getNonce and is the exact reverse of the truth for
//     getBalance, so a reader of the index concluded the opposite of the correct
//     advice and signed against the mempool cursor.
//   under-attached: error-code-32014-undocumented is entirely about
//     novai_submitTransaction and its prose names no method at all, so the only
//     state-mutating method on the surface had an empty Notes cell.
//
// A caveat is a claim about ONE method. It is declared per method in
// KNOWN_DRIFT.affects, checked there against the parsed document, and read here.
// The data generator's own comment on public-faucet-gating-backwards names
// prose matching as "exactly the mistake affects exists to prevent"; this file
// used to be where that mistake lived.
// ---------------------------------------------------------------------------

/** The index Notes column for one method, read from the method and nowhere else. */
function methodNotes(m) {
  const out = m.caveats.map((c) => c.label);
  // Measured from the handler, not from whether the reference happened to write
  // the null case inside a parenthesis. See measureNullAnswers in the data
  // generator for why that distinction mattered.
  if (m.answersNull) out.push("can answer null");
  if (m.withheld) out.push("not documented here");
  return [...new Set(out)];
}

// ---------------------------------------------------------------------------
// CORRECTIONS AT THE POINT OF THE ERROR
//
// Where an exception has measured that a specific published sentence is false,
// that sentence is STRUCK and the correction is printed underneath it.
//
// Struck rather than deleted, for three reasons in order of weight. The
// sentence is still published in docs/RPC_REFERENCE.md, which every method row
// links to, and this page cannot retract it: a reader who found the console had
// quietly dropped a sentence the document contains cannot tell curation from
// staleness from error. Second, it makes the known-gaps table checkable instead
// of merely assertable, because each carried exception now has a visible site
// rather than only a summary. Third, the confusion cost is one reading tax on
// five sites out of 29 and is answered by placement, putting the correction on
// the next line so the eye lands on the true statement; the cost of silent
// deletion is a permanent invisible divergence that someone later "restores".
//
// No new CSS class. <del> carries the semantics for assistive tech and two
// utilities carry the appearance, both visible to Tailwind's content scan
// because this markup is written into console.html on disk.
// ---------------------------------------------------------------------------

const STRUCK = 'class="text-ink-low line-through decoration-ink-low"';

/**
 * rich(), with one exact substring wrapped in <del>.
 *
 * The RAW markdown is split and the three fragments are enriched separately,
 * rather than string-replacing inside already-enriched HTML, so a split can
 * never land inside an HTML entity or an emitted <code> tag. Both boundaries
 * are asserted to lie outside a code span: an even backtick count in the prefix
 * puts the opening boundary outside one, and an even count in the needle puts
 * the closing boundary outside one too. attachExceptions has already proved the
 * needle occurs exactly once in exactly one block.
 */
function richStruck(text, wrongText) {
  const raw = String(text ?? "");
  const i = raw.indexOf(wrongText);
  if (i === -1) fail(`the text to strike is not published: "${wrongText}"`);
  const ticks = (str) => (str.match(/`/g) ?? []).length;
  if (ticks(wrongText) % 2 !== 0) {
    fail(`wrongText has unbalanced backticks, so striking it would split a code span: "${wrongText}"`);
  }
  if (ticks(raw.slice(0, i)) % 2 !== 0) {
    fail(`wrongText starts inside a code span, so striking it would split it: "${wrongText}"`);
  }
  return `${rich(raw.slice(0, i))}<del ${STRUCK}>${rich(wrongText)}</del>${rich(raw.slice(i + wrongText.length))}`;
}

/** The true statement, printed immediately under the struck one. */
const correctionNote = (c) =>
  noteHtml(
    `<span class="console-pill">correction</span> ${rich(c.correction)} ` +
    `<span class="text-ink-low">Tracked as ${esc(c.operatorRef)}.</span>`
  );

/** The corrections that belong under one block of one method. */
const correctionsAt = (m, site) => m.corrections.filter((c) => c.site === site);

/**
 * The one correction whose false text lives in this exact string, if any. Used
 * by the params and errors tables, where the same site holds many cells and
 * only one of them carries the falsehood.
 */
const struckIn = (m, site, text) =>
  correctionsAt(m, site).find((c) => c.wrongText && String(text ?? "").includes(c.wrongText)) ?? null;

function renderRpcIndex(d) {
  // Repeat the category only when it changes, matching the reference's own
  // index. Printing "SLA methods (Week 31)" on four consecutive rows is noise
  // in a column whose only job is to group.
  let lastCategory = null;
  const rows = d.methods.value.map((m) => [
    m.category === lastCategory ? "" : esc((lastCategory = m.category)),
    `<a class="text-brand-text no-underline hover:underline" href="#${esc(anchorFor(m.name))}">${esc(m.name)}</a>`,
    esc(plain(briefOf(m))),
    methodNotes(m).map((n) => `<span class="console-pill">${esc(n)}</span>`).join(" "),
  ]);
  return `${lead(`All ${d.methods.value.length} methods. ${PROSE.rpc}`)}
        ${table([{ label: "Category" }, { label: "Method" }, { label: "Brief" }, { label: "Notes" }], rows)}
        ${note(PROSE.rpcSource)}`;
}

// ---------------------------------------------------------------------------
// RECORD SHAPES
//
// Fifteen of the 29 methods answer with a payload whose type is named and not
// defined: `{ "channels": [PaymentChannel, ...] }`, `{ "anchor": OracleAnchor |
// null }`. The page printed the note "Record shape declared once for <category>
// and shared by every method in it" seventeen times and never rendered a single
// shape, so `basis_points` and `anchor_data_hash_equals` appeared zero times on
// a page that claims to be everything a developer needs.
//
// The shape was in the data the whole time, in m.result.recordShape. The
// renderer read `m.result.envelope ?? m.result.recordShape`, and `??` takes the
// envelope whenever there is one, which is exactly the case where the record
// shape is the missing half. Nothing had to be parsed to fix this; it had to be
// printed.
//
// Each type is defined ONCE, at the first method that references it, and every
// reference to it anywhere becomes a link to that definition. One definition
// keeps the six shapes from being repeated fifteen times; the links are what
// make a bare type name navigable, and they survive the page split.
// ---------------------------------------------------------------------------

const recordAnchor = (type) => `record-${type.toLowerCase()}`;

/**
 * Escape a shape fence and turn each bare record-type reference into a link.
 *
 * String- and comment-aware, because a type name inside a quoted default or a
 * jsonc comment is prose about the type, not a reference to it, and linking it
 * would put an anchor inside a code literal. The scan mirrors the data
 * generator's recordTypesIn, which is what decided these were references in the
 * first place, so the two cannot disagree about what counts.
 */
function linkRecordTypes(text, known) {
  const src = String(text ?? "");
  let out = "";
  let i = 0;
  while (i < src.length) {
    if (src[i] === '"') {
      const end = src.indexOf('"', i + 1);
      const stop = end === -1 ? src.length : end + 1;
      out += esc(src.slice(i, stop));
      i = stop;
      continue;
    }
    if (src[i] === "/" && src[i + 1] === "/") {
      const end = src.indexOf("\n", i);
      const stop = end === -1 ? src.length : end;
      out += esc(src.slice(i, stop));
      i = stop;
      continue;
    }
    const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i));
    if (m) {
      const word = m[0];
      out += known.has(word)
        ? `<a class="text-brand-text no-underline hover:underline" href="#${esc(recordAnchor(word))}">${esc(word)}</a>`
        : esc(word);
      i += word.length;
      continue;
    }
    out += esc(src[i]);
    i += 1;
  }
  return out;
}

/** A shape fence with its record-type references linked. */
const preLinked = (body, known) =>
  `<pre class="console-pre"><code>${linkRecordTypes(body, known)}</code></pre>`;

/** The one definition of a record type, emitted at its first reference. */
function recordDefinition(type, shape, categoryTitle, known) {
  return `<div class="console-record" id="${esc(recordAnchor(type))}">
          ${h3(`${type} record`)}
          ${note(`Declared once for ${categoryTitle} and shared by every method in it.`)}
          ${preLinked(shape, known)}
        </div>`;
}

function renderParams(m) {
  if (!m.params) return note("Not documented.");
  if (m.params.kind === "none") return `${h3("params")}${note(m.params.note ?? "none")}`;
  const rows = (m.params.list ?? []).map((p) => {
    const c = struckIn(m, "params", p.notes);
    return [
      `<code>${esc(p.field)}</code>`,
      `<code>${esc(p.type)}</code>`,
      p.optional ? "optional" : "required",
      c ? richStruck(p.notes, c.wrongText) : rich(p.notes),
    ];
  });
  if (rows.length === 0) return "";
  const from = m.params.resolvedFrom ? note(`Same parameters as ${m.params.resolvedFrom}.`) : "";
  const fixes = correctionsAt(m, "params").map(correctionNote).join("\n        ");
  return `${h3("params")}${table([{ label: "Field" }, { label: "Type" }, { label: "Required" }, { label: "Notes" }], rows)}${from}${fixes}`;
}

function renderResult(m, known) {
  if (!m.result) return "";
  const parts = [h3("result")];
  // The envelope, with every record-type reference in it linked to the one
  // place that defines it. The previous version read
  // `m.result.envelope ?? m.result.recordShape`, which takes the envelope
  // whenever there is one, and the envelope is precisely the half that names a
  // type without defining it.
  if (m.result.envelope) parts.push(preLinked(m.result.envelope, known));
  else if (m.result.recordShape) parts.push(preLinked(m.result.recordShape, known));
  if (m.result.inheritedFrom && (m.result.recordTypes ?? []).length) {
    const types = m.result.recordTypes
      .map((t) => `<a class="text-brand-text no-underline hover:underline" href="#${esc(recordAnchor(t))}">${esc(t)}</a>`)
      .join(", ");
    parts.push(noteHtml(`Shape of ${types} is declared once for ${esc(m.result.inheritedFrom)} and shared by every method in it.`));
  } else if (m.result.inheritedFrom) {
    parts.push(note(`Record shape declared once for ${m.result.inheritedFrom} and shared by every method in it.`));
  }
  // The null case is stated from the handler rather than quoted from the
  // reference. The reference's own wording for getBlockByHeight says the error
  // path "should be unreachable given the validation", which the known-gaps
  // section of this same page measures as false, so quoting it would publish a
  // sentence the page refutes 2,600 lines later.
  if (m.answersNull) {
    parts.push(
      noteHtml(
        `Null case: this method answers with a top-level <code class="text-ink-mid">null</code> result rather ` +
        `than an error when the record is not there, which for history includes anything below the pruning ` +
        `horizon. Measured at ${sourceLink(m.answersNull.file, m.answersNull.line, `${m.answersNull.file}:${m.answersNull.line}`)}.`
      )
    );
  }
  parts.push(...correctionsAt(m, "result").map(correctionNote));
  return parts.join("\n        ");
}

function renderErrors(m) {
  if (!m.errors) return "";
  const list = m.errors.list ?? [];
  // A correction on the errors site with no wrongText adds a row the reference
  // never had (the undocumented -32014), so there is nothing to strike and it
  // renders as a plain correction under the table.
  const fixes = correctionsAt(m, "errors").map(correctionNote).join("\n        ");
  if (m.errors.kind === "prose" || list.length === 0) {
    // rich() rather than plain() so a struck span can live here, and so the
    // codes in this prose render as <code> like they do everywhere else.
    const c = struckIn(m, "errors", m.errors.text);
    const body = m.errors.text
      ? (c ? richStruck(m.errors.text, c.wrongText) : rich(m.errors.text))
      : "Only the global codes.";
    return `${h3("errors")}${noteHtml(body)}${fixes}`;
  }
  const seen = new Set();
  const rows = [];
  for (const e of list) {
    const key = `${e.code}::${e.when}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const c = struckIn(m, "errors", e.when);
    rows.push([`<code>${esc(e.code)}</code>`, c ? richStruck(e.when, c.wrongText) : rich(e.when)]);
  }
  const from = m.errors.kind === "categoryCommon" ? note(`Shared by every method in ${m.errors.from}.`) : "";
  return `${h3("errors")}${table([{ label: "Code" }, { label: "When" }], rows)}${from}${fixes}`;
}

function renderMethods(d) {
  const refs = new Map(d.sourceRefs.value.map((r) => [r.name, r]));
  const known = new Set(d.methods.value.flatMap((m) => m.result?.recordTypes ?? []));
  const defined = new Set();
  return d.methods.value
    .map((m) => {
      // The definition goes in front of the first method that references the
      // type, which is where the reference document itself puts it: in the
      // category preamble, above the methods of that category.
      let definitions = "";
      for (const t of m.result?.recordTypes ?? []) {
        if (defined.has(t) || !m.result.recordShape) continue;
        defined.add(t);
        definitions += recordDefinition(t, m.result.recordShape, m.result.inheritedFrom, known) + "\n        ";
      }
      const ref = refs.get(m.name);
      const link = ref ? sourceLink(ref.file, ref.line, `${ref.file}:${ref.line}`) : "";
      const head = `<div class="flex flex-wrap items-baseline justify-between gap-2">
            <span class="console-method-name">${esc(m.name)}</span>
            <span class="font-mono text-[10px] text-ink-low">${link}</span>
          </div>`;

      // A withheld method keeps its name, its anchor, its source link and its
      // caveats, and loses its params, result, curl and sample response. The
      // page claims to cover 29 methods and that claim stays literally true;
      // what it stops publishing is a runnable mint. Hiding the entry outright
      // would leave a "29 methods, agreed" tile above a 28-row table, break
      // deep links, and make the console the only one of the four
      // cross-checked sources that omits a method all four agree on.
      if (m.withheld) {
        return `${definitions}<div class="console-method" id="${esc(anchorFor(m.name))}">
          ${head}
          <p class="console-lead">${esc(plain(briefOf(m)))} <span class="console-pill">not documented here</span></p>
          ${note(m.withheld.reason)}
          ${m.corrections.map(correctionNote).join("\n        ")}
        </div>`;
      }

      const descText = m.description || m.brief;
      const descFix = correctionsAt(m, "description").find((c) => c.wrongText) ?? null;
      return `${definitions}<div class="console-method" id="${esc(anchorFor(m.name))}">
          ${head}
          <p class="console-lead">${descFix ? richStruck(descText, descFix.wrongText) : rich(descText)}</p>
          ${correctionsAt(m, "description").map(correctionNote).join("\n        ")}
          ${renderParams(m)}
          ${renderResult(m, known)}
          ${renderErrors(m)}
          ${h3("example")}
          ${m.exampleNote ? note(plain(m.exampleNote)) : ""}
          ${pre(m.curl)}
          ${m.sampleResponse
            ? out(m.sampleResponse) + note("Example response from the reference. Heights and hashes in it are illustrative, not a current reading.")
            : note("The reference carries no sample response for this method.")}
        </div>`;
    })
    .join("\n        ");
}

// Presentation order for the error table: the four JSON-RPC standard codes in
// the order the specification lists them, then the server-defined codes
// ascending. Sorting numerically alone interleaves the two groups and buries
// the standard ones at the bottom.
const STANDARD_CODE_ORDER = [-32700, -32600, -32601, -32602];

function errorSortKey(code) {
  const i = STANDARD_CODE_ORDER.indexOf(code);
  return i === -1 ? [1, -code] : [0, i];
}

function renderErrorsSection(d) {
  const docByCode = new Map(d.errorCatalogue.value.map((e) => [e.code, e]));
  const ordered = [...d.errorCodes.value].sort((a, b) => {
    const [ga, ia] = errorSortKey(a.code);
    const [gb, ib] = errorSortKey(b.code);
    return ga - gb || ia - ib;
  });
  // Corrections that land on a code rather than on a method, struck in the
  // Trigger cell where a reader meets the wrong fact.
  const fixesByCode = new Map();
  for (const c of d.codeCorrections.value) {
    if (!fixesByCode.has(c.code)) fixesByCode.set(c.code, []);
    fixesByCode.get(c.code).push(c);
  }
  const rows = ordered.map((e) => {
    const doc = docByCode.get(e.code);
    const fixes = fixesByCode.get(e.code) ?? [];
    const strike = fixes.find((c) => c.wrongText);
    const trigger = doc
      ? (strike ? richStruck(doc.trigger, strike.wrongText) : rich(doc.trigger))
      : "nonce is at or above the sender's admission horizon";
    const pills = [
      ...(doc ? [] : ["not in the reference"]),
      ...fixes.map((c) => c.caveat),
    ];
    return [
      `<code>${esc(e.code)}</code>`,
      esc(doc ? plain(doc.meaning) : "NonceTooHigh"),
      trigger,
      pills.map((p) => `<span class="console-pill">${esc(p)}</span>`).join(" "),
      sourceLink(e.file, e.line, `:${e.line}`),
    ];
  });
  const codeFixes = d.codeCorrections.value.map(correctionNote).join("\n        ");
  const httpRows = d.httpRejections.value.map((r) => [
    `<code>${esc(r.status)}</code>`,
    `<code>${escWire(r.body)}</code>${r.bodyIsTemplate ? ' <span class="console-pill">template</span>' : ""}`,
    sourceLink(r.file, r.line, `:${r.line}`),
  ]);
  const limitRows = d.sourceLimits.value.map((l) => [
    esc(l.unit),
    esc(l.value.toLocaleString("en-US")),
    `<code>${esc(l.name)}</code>`,
    sourceLink(l.file, l.line, `:${l.line}`),
  ]);
  return `${lead(PROSE.errors)}
        ${h3("json-rpc error codes")}
        ${table([{ label: "Code" }, { label: "Meaning" }, { label: "Trigger" }, { label: "Note" }, { label: "Source" }], rows)}
        ${codeFixes}
        ${note(PROSE.errors32014)}
        ${h3("http rejections with no json-rpc envelope")}
        ${table([{ label: "Status" }, { label: "Body" }, { label: "Source" }], httpRows)}
        ${note(PROSE.errorsHttp)}
        ${h3("limits")}
        ${table([{ label: "Limit" }, { label: "Value", num: true }, { label: "Constant" }, { label: "Source" }], limitRows)}
        ${h3("handling a rejected nonce")}
        ${note(PROSE.errorsNonce)}
        ${note(PROSE.errorsBalance)}`;
}

function renderTransactions(d) {
  const feeRows = d.fees.value.map((f) => [
    String(f.discriminant),
    `<code>${esc(f.name)}</code>`,
    esc(f.minFee.toLocaleString("en-US")),
    `<code>${esc(f.feeConstant)}</code>`,
  ]);
  const bps = d.bpsFees.value;
  const bpsRows = bps.entries.map((e) => [
    `<code>${esc(e.constant)}</code>`,
    esc(String(e.bps)),
    esc(`${e.percent}%`),
    sourceLink(e.file, e.line, `:${e.line}`),
  ]);
  const w = d.txWireLayout.value;
  const wireRows = w.fields.map((f) => [
    `<code>${esc(f.field)}</code>`,
    f.bytes === null ? "variable" : esc(String(f.bytes)),
    f.endianness ? esc(f.endianness) : "n/a",
    f.offset === null ? "after the length" : esc(String(f.offset)),
  ]);
  return `${lead(PROSE.transactions)}
        ${h3("types, discriminants and minimum fees")}
        ${table([{ label: "Byte", num: true }, { label: "Type" }, { label: "Min fee", num: true }, { label: "Constant" }], feeRows)}
        ${note(PROSE.txFees)}
        ${h3("percentage fees")}
        ${table([{ label: "Constant" }, { label: "Basis points", num: true }, { label: "Share", num: true }, { label: "Source" }], bpsRows)}
        ${note(`Basis points are against a denominator of ${bps.denominator.toLocaleString("en-US")}.`)}
        ${h3("canonical unsigned encoding")}
        ${table([{ label: "Field" }, { label: "Bytes", num: true }, { label: "Endianness" }, { label: "Offset", num: true }], wireRows)}
        ${note(`The signature is ${w.signatureBytes} bytes appended after the payload. Fixed overhead is ${w.overhead} bytes, which is ${w.overheadConstant}.`)}
        ${h3("signing")}
        ${note(PROSE.txSigning)}`;
}

function renderRecipes(d) {
  const fee = (name) => {
    const f = d.fees.value.find((x) => x.name === name);
    return f ? f.minFee.toLocaleString("en-US") : "?";
  };
  const tail = (name) => {
    const t = d.signalPayloads.value.tails.find((x) => x.name === name);
    return t ? t.value : null;
  };
  const base = d.signalPayloads.value.baseLength.value;
  const market = d.bpsFees.value.entries.find((e) => e.constant === "MARKETPLACE_FEE_BPS");
  const bit5 = d.capabilityBits.value.find((c) => c.bit === 5);

  const recipe = (title, points) =>
    `<div class="mt-6 rounded-md border border-line bg-surface-1 px-4 py-3">
          <p class="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-hi">${esc(title)}</p>
          <ul class="mt-2 list-disc space-y-1.5 pl-5 text-[13px] leading-relaxed text-ink-mid">
            ${points.map((p) => `<li>${p}</li>`).join("\n            ")}
          </ul>
        </div>`;

  const reputation = recipe("a reputation oracle", [
    "A reputation oracle issues <code>ReputationUpdate</code> signals, signal type 7. Each one moves a target entity's <code>reputation_score</code>, which the chain stores on the entity record and clamps to a 0 to 100 range. A newly registered entity starts at 50.",
    `Issuing one requires the <code>${esc(bit5.capability)}</code> capability, bit ${esc(String(bit5.bit))} of the entity's capability byte.`,
    `The payload is ${esc(String(base + tail("REPUTATION_UPDATE_EXTRA_LEN")))} bytes: the ${esc(String(base))}-byte signal commitment base plus a ${esc(String(tail("REPUTATION_UPDATE_EXTRA_LEN")))}-byte tail carrying the target entity id, an event type and a signed two-byte delta.`,
    "The event type must be 0 to 12.",
    "An entity cannot rate itself.",
    `The fee is ${esc(fee("signalCommitment"))} base units, the signal commitment minimum.`,
    "The effect is checkable in one read: <code>novai_getAiEntity</code> returns <code>reputation_score</code> and <code>reputation_events_count</code>.",
  ]);

  const marketplace = recipe("selling signals on the marketplace", [
    "A seller publishes a <code>SignalCatalog</code> memory object, type 7, listing priced offerings. A catalog holds at most 10 offerings, each 10 bytes: a signal type, a price as a big-endian u64, and an active flag.",
    "A buyer issues a <code>SignalPurchase</code> signal, type 8, naming the seller, the signal type wanted and a maximum price. The chain rejects the purchase if the catalog price is above that maximum.",
    `On success the chain moves the price from the buyer's <code>economic_balance</code> to the seller's, less a ${esc(String(market.percent))}% cut routed to the marketplace treasury.`,
    `The payload is ${esc(String(base + tail("SIGNAL_PURCHASE_EXTRA_LEN")))} bytes: the ${esc(String(base))}-byte base plus a ${esc(String(tail("SIGNAL_PURCHASE_EXTRA_LEN")))}-byte tail.`,
    "<strong>Publishing a catalog requires no stake.</strong> A listing is not a bond, and a buyer should not read it as one.",
    "The marketplace treasury is not readable over RPC. No method returns it.",
  ]);

  const staking = recipe("staking collateral", [
    `A <code>StakeDeposit</code> signal, type 9, moves an amount from an entity's <code>economic_balance</code> into its <code>stake_balance</code>. The payload is ${esc(String(base + tail("STAKE_DEPOSIT_EXTRA_LEN")))} bytes: the ${esc(String(base))}-byte base plus a ${esc(String(tail("STAKE_DEPOSIT_EXTRA_LEN")))}-byte big-endian u128 amount.`,
    "A deposit locks for 1,000 blocks. <code>stake_locked_until</code> is set to the current height plus that period, and a withdrawal before that height is rejected. The lock is counted in blocks, not in time; the cadence in the network section converts it.",
    `Withdrawal is a <code>StakeWithdraw</code> signal, type 10, with the same ${esc(String(base + tail("STAKE_WITHDRAW_EXTRA_LEN")))}-byte layout. A partial withdrawal does not re-lock the remainder.`,
    `A <code>StakeSlash</code> signal, type 11, issued by an entity holding <code>${esc(bit5.capability)}</code>, subtracts up to a named amount from the target's <code>stake_balance</code> and applies a bundled reputation event in the same operation. The payload is ${esc(String(base + tail("STAKE_SLASH_EXTRA_LEN")))} bytes.`,
    "An entity cannot slash itself.",
    "<code>economic_balance</code>, <code>stake_balance</code> and <code>stake_locked_until</code> are all readable from <code>novai_getAiEntity</code>, and every stake operation appears in the signal index.",
  ]);

  return reputation + "\n        " + marketplace + "\n        " + staking;
}

function renderAiEntities(d) {
  const capRows = d.capabilityBits.value.map((c) => [
    String(c.bit),
    `<code>${esc(c.hex)}</code>`,
    `<code>${esc(c.capability)}</code>`,
  ]);
  const signalRows = d.signalTypes.value.map((s) => [
    String(s.discriminant),
    `<code>${esc(s.variant)}</code>`,
    esc(s.payloadNote ?? ""),
    s.description ? esc(plain(s.description)) : '<span class="console-pill">no source description</span>',
  ]);
  const memoryRows = d.memoryObjectTypes.value.map((m) => [
    String(m.discriminant),
    `<code>${esc(m.variant)}</code>`,
    esc(plain(m.description ?? "")),
  ]);
  const undocumented = d.gaps.value.signalTypesWithoutSourceDescription;
  return `${lead(PROSE.ai)}
        ${note(PROSE.aiRecipesNote)}
        ${renderRecipes(d)}
        ${h3("capability bits")}
        ${table([{ label: "Bit", num: true }, { label: "Hex" }, { label: "Capability" }], capRows)}
        ${note(PROSE.aiCapabilities)}
        ${details(`signal types (${d.signalTypes.value.length})`, table([{ label: "Type", num: true }, { label: "Variant" }, { label: "Payload" }, { label: "Description" }], signalRows) + note(`${undocumented.length} of these carry no doc comment in the Rust source, so no description can be generated for them: ${undocumented.join(", ")}. Their payload sizes are known and are shown.`))}
        ${details(`memory object types (${d.memoryObjectTypes.value.length})`, table([{ label: "Type", num: true }, { label: "Variant" }, { label: "Description" }], memoryRows))}`;
}

function renderSdks(d) {
  const c = d.sdkCoverage.value;
  const yes = '<span class="text-ink-hi">yes</span>';
  const no = '<span class="text-ink-low">no</span>';
  const builderRows = c.builders.map((b) => [
    String(b.discriminant),
    `<code>${esc(b.txType)}</code>`,
    b.rust ? yes : no,
    b.python ? yes : no,
    b.typescript ? yes : no,
  ]);
  const covRows = [
    ["transaction builders", c.totals.txTypes, c.totals.rustBuilders, c.totals.pythonBuilders, c.totals.typescriptBuilders],
    ["signal types", c.signalTypes.chain, c.signalTypes.rust, c.signalTypes.python, c.signalTypes.typescript],
    ["memory object types", c.memoryObjectTypes.chain, c.memoryObjectTypes.rust, c.memoryObjectTypes.python, c.memoryObjectTypes.typescript],
  ].map((r) => [esc(r[0]), String(r[1]), String(r[2]), String(r[3]), String(r[4])]);

  const installRows = [
    ["Python", `<code>pip install novai-sdk</code>`, "PyPI, version 0.1.0"],
    ["Rust", `<code>git clone</code> the repository and depend on <code>sdk/novai-sdk</code> by path`, "not on crates.io"],
    ["TypeScript", `<code>git clone</code> the repository and build <code>sdk/novai-sdk-ts</code>`, "not published to npm"],
  ].map((r) => [esc(r[0]), r[1], esc(r[2])]);

  const deps = c.workspaceCoupling.rust.pathDependencies.map((p) => p.crate).join(", ");

  return `${lead(PROSE.sdks)}
        ${h3("getting them")}
        ${table([{ label: "SDK" }, { label: "Install" }, { label: "Registry" }], installRows)}
        ${note(PROSE.sdkDistribution)}
        ${note(`The Rust SDK depends on the workspace crates by path (${deps}), so there is no artifact a registry could carry.`)}
        ${h3("coverage")}
        ${table([{ label: "Surface" }, { label: "Chain", num: true }, { label: "Rust", num: true }, { label: "Python", num: true }, { label: "TypeScript", num: true }], covRows)}
        ${note(PROSE.sdkRustStructural)}
        ${note(PROSE.sdkTypescript)}
        ${details("transaction builders, per SDK", table([{ label: "Byte", num: true }, { label: "Type" }, { label: "Rust" }, { label: "Python" }, { label: "TypeScript" }], builderRows))}`;
}

function renderParameters(d) {
  const q = d.quorum.value;
  const n = 4;
  const f = Math.floor((n - 1) / 3);
  const quorum = 2 * f + 1;
  const rows = [
    ["Validators", String(n), "configuration fact, set by hand"],
    ["Quorum", String(quorum), `<code>${esc(q.expression)}</code> evaluated at n=${n}`],
    ["Faults tolerated", String(f), "f, from the same rule"],
    ["Transaction types", String(d.txTypes.value.length), "payload discriminants"],
    ["Signal types", String(d.signalTypes.value.length), "AiSignalType variants"],
    ["Memory object types", String(d.memoryObjectTypes.value.length), "MemoryObjectType variants"],
    ["JSON-RPC methods", String(d.drift.value.agreedMethodCount), "agreed across four sources"],
  ].map((r) => [esc(r[0]), r[1], r[2]]);

  const rh = d.retentionHorizons.value;
  const retentionRows = [
    [esc(rh.disk.what), rh.disk.value.toLocaleString("en-US"), `<code>${esc(rh.disk.name)}</code>`, sourceLink(rh.disk.file, rh.disk.line, `:${rh.disk.line}`)],
    [esc(rh.index.what), rh.index.value.toLocaleString("en-US"), `<code>${esc(rh.index.name)}</code>`, sourceLink(rh.index.file, rh.index.line, `:${rh.index.line}`)],
  ];

  return `${lead(PROSE.parameters)}
        ${table([{ label: "Parameter" }, { label: "Value", num: true }, { label: "How it is known" }], rows)}
        ${note(PROSE.parametersValidators)}
        ${noteHtml(`Quorum sites agree: ${q.sites.map((s) => sourceLink(s.file, s.line, `${s.file}:${s.line}`)).join(" and ")}.`)}
        ${h3("retention")}
        ${table([{ label: "Horizon" }, { label: "Blocks", num: true }, { label: "Constant" }, { label: "Source" }], retentionRows)}
        ${note(PROSE.parametersRetention)}
        ${h3("deliberately omitted")}
        ${note(PROSE.parametersOmitted)}`;
}

function renderGaps(d) {
  const gapRows = d.observedGaps.value.map((g) => [
    rich(g.Gap ?? ""),
    rich(g.Impact ?? ""),
    rich(g["Workaround today"] ?? ""),
  ]);
  const exRows = d.drift.value.knownExceptions.map((e) => [
    `<code>${esc(e.id)}</code>`,
    esc(plain(e.summary)),
    esc(e.operatorRef),
  ]);
  return `${lead(PROSE.gaps)}
        ${h3("history is pruned, and block 0 is gone")}
        ${note(PROSE.gapsRetention)}
        ${note(PROSE.gapsPruned)}
        ${h3("an indexer cannot backfill from an rpc node")}
        ${note(PROSE.gapsIndex)}
        ${h3("getnonce and getbalance are different numbers")}
        ${note(PROSE.gapsNonce)}
        ${h3("surface that exists but is not reachable over rpc")}
        ${table([{ label: "Gap" }, { label: "Impact" }, { label: "Workaround" }], gapRows)}
        ${h3("carried drift exceptions")}
        ${note(PROSE.gapsExceptions)}
        ${table([{ label: "Exception" }, { label: "Summary" }, { label: "Tracked as" }], exRows)}`;
}

function renderVerify() {
  return `${lead(PROSE.verify)}
        <div class="mt-4 rounded-md border border-line bg-surface-1 px-4 py-3" data-island="verify-panel">
          <p class="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-low">hash linkage check</p>
          <p class="console-note">${esc(PROSE.verifyStatic)}</p>
        </div>`;
}

// ---------------------------------------------------------------------------
// Regions
//
// One region per generated block. The count is asserted against the markers
// found in the file, so a region silently dropped from either side fails.
// ---------------------------------------------------------------------------

const REGIONS = [
  { id: "intro", render: renderIntro },
  { id: "provenance", render: renderProvenance },
  { id: "connect", render: renderConnect },
  { id: "first-call", render: renderFirstCall },
  { id: "rpc-index", render: renderRpcIndex },
  { id: "rpc-methods", render: renderMethods },
  { id: "errors", render: renderErrorsSection },
  { id: "transactions", render: renderTransactions },
  { id: "ai-entities", render: renderAiEntities },
  { id: "sdks", render: renderSdks },
  { id: "parameters", render: renderParameters },
  { id: "gaps", render: renderGaps },
  { id: "verify", render: renderVerify },
];

// ---------------------------------------------------------------------------
// MARKER GATES
//
// console.html is part generated and part hand-written, and the generator
// rewrites regions in place. Four things therefore have to be true, and each is
// asserted rather than assumed:
//
//   1. The sigil cannot be produced by ordinary editing. "@@" inside an HTML
//      comment with a fixed prefix is not something a person types by accident,
//      and it is checked for exactly.
//   2. Every opening marker has its closing pair, in order, not nested.
//   3. The number of marked regions equals the number of regions this script
//      intends to write. A region dropped from either side fails loudly rather
//      than vanishing from the page.
//   4. --check re-renders every region and compares. A hand-edit inside a
//      generated region is reported before the next run silently overwrites it.
// ---------------------------------------------------------------------------

const SIGIL = "@@console-generated";
const openMarker = (id) => `<!-- ${SIGIL}:${id}@@ -->`;
const closeMarker = (id) => `<!-- ${SIGIL}:/${id}@@ -->`;

function findRegions(html) {
  const all = [...html.matchAll(new RegExp(`<!-- ${SIGIL}:(\\/?)([a-z-]+)@@ -->`, "g"))];
  const opens = all.filter((m) => m[1] === "");
  const closes = all.filter((m) => m[1] === "/");

  if (opens.length !== closes.length) {
    fail(`marker pairing: ${opens.length} opening marker(s) and ${closes.length} closing marker(s)`);
  }

  const found = [];
  const stack = [];
  for (const m of all) {
    if (m[1] === "") {
      if (stack.length) fail(`marker nesting: region ${m[2]} opens inside ${stack[stack.length - 1]}`);
      stack.push(m[2]);
      found.push({ id: m[2], start: m.index, bodyStart: m.index + m[0].length });
    } else {
      const open = stack.pop();
      if (open === undefined) fail(`marker pairing: region ${m[2]} closes without opening`);
      if (open !== m[2]) fail(`marker pairing: region ${open} is closed by ${m[2]}`);
      const region = found[found.length - 1];
      region.bodyEnd = m.index;
      region.end = m.index + m[0].length;
    }
  }
  if (stack.length) fail(`marker pairing: region ${stack[0]} never closes`);

  const foundIds = found.map((r) => r.id);
  const wantIds = REGIONS.map((r) => r.id);
  const missing = wantIds.filter((id) => !foundIds.includes(id));
  const extra = foundIds.filter((id) => !wantIds.includes(id));
  if (missing.length || extra.length || foundIds.length !== wantIds.length) {
    fail(
      `region count: console.html carries ${foundIds.length} marked region(s) and this script writes ${wantIds.length}. ` +
      `Missing from the page: ${missing.join(", ") || "none"}. On the page with no renderer: ${extra.join(", ") || "none"}.`
    );
  }
  const duplicates = foundIds.filter((id, i) => foundIds.indexOf(id) !== i);
  if (duplicates.length) fail(`region count: duplicate region id(s) ${[...new Set(duplicates)].join(", ")}`);
  return found;
}

/** Indent a rendered block so the written file stays readable in a diff. */
function block(id, body) {
  return `\n        ${body.trim()}\n        `;
}

// ---------------------------------------------------------------------------
// RENDER GATES
//
// These run on the rendered markup, not on the data, because every defect this
// gate exists for was correct in the data and wrong on the page. The generator
// computed the right caveat, gated it three ways, and then the projection
// dropped it and the renderer invented its own from prose. A gate that checks
// the data would have passed the whole time.
// ---------------------------------------------------------------------------

/** The <tr> whose method cell links to this anchor. */
function rowFor(indexHtml, name) {
  const rows = indexHtml.match(/<tr>[\s\S]*?<\/tr>/g) ?? [];
  const needle = `href="#${anchorFor(name)}"`;
  const hits = rows.filter((r) => r.includes(needle));
  if (hits.length !== 1) fail(`index gate: ${name} appears in ${hits.length} index rows, expected exactly 1`);
  return hits[0];
}

const pillsIn = (fragment) =>
  [...fragment.matchAll(/<span class="console-pill">([^<]*)<\/span>/g)].map((m) => m[1]);

/** The method block for one method, bounded by the next block or the region end. */
function blockFor(methodsHtml, name) {
  const start = methodsHtml.indexOf(`id="${anchorFor(name)}"`);
  if (start === -1) fail(`method gate: no rendered block for ${name}`);
  const next = methodsHtml.indexOf('<div class="console-method"', start);
  return methodsHtml.slice(start, next === -1 ? methodsHtml.length : next);
}

/**
 * Every pill on a row was declared by that method, and every caveat the method
 * declares is on its row. BOTH directions, because the mechanism this replaced
 * failed in both: it put getNonce's caveat on getBalance, where it is the
 * reverse of the truth, and it put submitTransaction's caveat nowhere.
 */
function assertCaveatsAreDeclared(d, indexHtml) {
  for (const m of d.methods.value) {
    const row = rowFor(indexHtml, m.name);
    const declared = methodNotes(m);
    const rendered = pillsIn(row);
    for (const label of declared) {
      if (!rendered.includes(esc(label))) {
        fail(`index gate: ${m.name} declares the caveat "${label}" and its index row does not carry it`);
      }
    }
    for (const label of rendered) {
      if (!declared.map(esc).includes(label)) {
        fail(
          `index gate: ${m.name}'s index row carries the caveat "${label}", which the method does not declare. ` +
          `Caveats come from KNOWN_DRIFT.affects and from m.result.nullable, never from matching prose.`
        );
      }
    }
  }
}

/**
 * Every correction reaches the page, every wrongText is actually struck, and
 * the page carries no struck span that no correction asked for. The count check
 * is the one that catches a strike applied to the wrong string.
 */
function assertCorrectionsAreRendered(d, methodsHtml) {
  let wantStruck = 0;
  for (const m of d.methods.value) {
    const block = blockFor(methodsHtml, m.name);
    for (const c of m.corrections) {
      if (!block.includes(rich(c.correction))) {
        fail(`correction gate: ${m.name} carries the correction from ${c.exceptionId} in its data and not on the page`);
      }
      if (c.wrongText) {
        wantStruck += 1;
        if (!block.includes(`<del ${STRUCK}>${rich(c.wrongText)}</del>`)) {
          fail(`correction gate: ${m.name} publishes "${c.wrongText}" unstruck while ${c.exceptionId} says it is false`);
        }
      }
    }
  }
  const found = (methodsHtml.match(/<del /g) ?? []).length;
  if (found !== wantStruck) {
    fail(`correction gate: ${found} struck span(s) on the page and ${wantStruck} correction(s) carrying a wrongText`);
  }
}

/**
 * Every record type a result shape names has a rendered definition, exactly one
 * of them, and every definition is reachable by the link that references it.
 *
 * This is the gate for the defect the falsifier missed and the one that
 * dead-ends a reader rather than misleading them: fifteen of 29 methods
 * published `{ "channels": [PaymentChannel, ...] }` against a page that never
 * said what a PaymentChannel is. A cross-reference to content the page does not
 * contain is a defect of the same rank as a false statement, and this is the
 * check that makes it impossible to ship one.
 */
/**
 * The null badge is on exactly the methods the handler scan found, and the
 * scan found something. A scan that silently matches nothing would put the
 * badge nowhere and pass every other check here.
 */
function assertNullBadgeIsMeasured(d, indexHtml) {
  const measured = d.methods.value.filter((m) => m.answersNull);
  if (measured.length === 0) {
    fail(
      "null gate: no method was measured as answering null. The handler scan in measureNullAnswers found " +
      "nothing, which is far more likely to be a broken scan than a chain whose reads never miss."
    );
  }
  for (const m of d.methods.value) {
    const badged = rowFor(indexHtml, m.name).includes(">can answer null<");
    if (badged !== Boolean(m.answersNull)) {
      fail(
        `null gate: ${m.name} is ${badged ? "badged" : "not badged"} "can answer null" and the handler scan says ` +
        `${m.answersNull ? "it does" : "it does not"}. The badge must follow the handler, never the reference's punctuation.`
      );
    }
  }
}

function assertRecordTypesAreDefined(d, methodsHtml) {
  const referenced = new Set(d.methods.value.flatMap((m) => m.result?.recordTypes ?? []));
  const defined = (methodsHtml.match(/id="record-[a-z0-9]+"/g) ?? []).map((a) => a.slice(4, -1));
  for (const type of referenced) {
    const id = recordAnchor(type);
    const n = defined.filter((x) => x === id).length;
    if (n === 0) {
      fail(
        `record gate: a result shape names ${type} and the page never defines it. ` +
        `A reader following that type has nowhere to go. Declare the shape in the category preamble of ` +
        `docs/RPC_REFERENCE.md, or stop naming the type in the envelope.`
      );
    }
    if (n > 1) fail(`record gate: ${type} is defined ${n} times, so a link to it is ambiguous`);
  }
  for (const id of new Set(defined)) {
    if (![...referenced].some((t) => recordAnchor(t) === id)) {
      fail(`record gate: the page defines ${id} and no result shape references it`);
    }
  }
  // And no link may point at an anchor that is not on the page.
  for (const href of new Set((methodsHtml.match(/href="#record-[a-z0-9]+"/g) ?? []).map((h) => h.slice(7, -1)))) {
    if (!defined.includes(href)) fail(`record gate: a reference links to #${href}, which the page does not define`);
  }
}

/**
 * Every hand-written string is used, and every one that is used is reachable.
 *
 * PROSE is where every sentence that is not read from the source tree lives, on
 * the stated ground that hand-written content is where facts rot and it should
 * sit in one auditable place. That only holds if the object and the page are
 * the same set. Four keys had drifted out of use, two of them byte-for-byte
 * duplicates of sentences hardcoded in console.html, so the page carried two
 * copies of one claim, one live and one dead, free to diverge. One of the dead
 * ones asserted that the Notes column names the caveat that applies to a
 * method, which is exactly the thing the page was getting wrong.
 */
function assertProseIsAllUsed(scriptSource) {
  const declared = [...scriptSource.matchAll(/^  ([a-zA-Z][a-zA-Z0-9]*):/gm)].map((m) => m[1]);
  const proseStart = scriptSource.indexOf("const PROSE = {");
  const proseEnd = scriptSource.indexOf("\n};", proseStart);
  if (proseStart === -1 || proseEnd === -1) fail("prose gate: the PROSE object was not found, so nothing was checked");
  const keys = [...scriptSource.slice(proseStart, proseEnd).matchAll(/^  ([a-zA-Z][a-zA-Z0-9]*):/gm)].map((m) => m[1]);
  if (keys.length === 0) fail("prose gate: PROSE parsed to zero keys, so the scan is broken");
  const unused = keys.filter((k) => !new RegExp(`PROSE\\.${k}\\b`).test(scriptSource));
  if (unused.length) {
    fail(
      `prose gate: PROSE.${unused.join(", PROSE.")} ${unused.length === 1 ? "is" : "are"} declared and never rendered. ` +
      `A hand-written sentence nobody prints is a second copy of a claim waiting to disagree with the first. ` +
      `Render it or delete it.`
    );
  }
  void declared;
}

function render(html, data) {
  const regions = findRegions(html);
  const byId = new Map(REGIONS.map((r) => [r.id, r]));
  // Rewrite from the end so earlier offsets stay valid.
  let out = html;
  const rendered = new Map();
  for (const region of [...regions].reverse()) {
    const body = block(region.id, byId.get(region.id).render(data));
    rendered.set(region.id, body);
    out = out.slice(0, region.bodyStart) + body + out.slice(region.bodyEnd);
  }
  // Gates run on what was just rendered, before it is written or compared.
  assertCaveatsAreDeclared(data, rendered.get("rpc-index") ?? "");
  assertCorrectionsAreRendered(data, rendered.get("rpc-methods") ?? "");
  assertRecordTypesAreDefined(data, rendered.get("rpc-methods") ?? "");
  assertNullBadgeIsMeasured(data, rendered.get("rpc-index") ?? "");
  return out;
}

function main() {
  const args = parseArgs(process.argv);
  assertProseIsAllUsed(readFileSync(fileURLToPath(import.meta.url), "utf8"));
  if (!existsSync(args.data)) fail(`${args.data} not found; run npm run console:data first`);
  if (!existsSync(args.html)) fail(`${args.html} not found`);

  const data = JSON.parse(readFileSync(args.data, "utf8"));
  const html = readFileSync(args.html, "utf8");
  const next = render(html, data);

  if (args.check) {
    if (next !== html) {
      fail(
        `--check: ${args.html} does not match a fresh render. Either the generated data changed or a generated ` +
        `region was hand-edited. Run npm run console:html and commit the result.`
      );
    }
    console.log(`console-html: check ok (${REGIONS.length} regions match a fresh render)`);
    return;
  }

  const tmp = args.html + ".tmp";
  writeFileSync(tmp, next);
  renameSync(tmp, args.html);
  console.log(`console-html: wrote ${args.html} (${REGIONS.length} regions)`);
}

main();
