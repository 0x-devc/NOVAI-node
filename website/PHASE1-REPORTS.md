# Gate C4, Phase 1 reports

Three specialised agents plus my own orientation measurements, run 2026-08-30 against the
frozen C3 console. Written down rather than held in context: the previous session lost its
agent output to a usage limit, and the reports are the only durable record of what was
attacked and what held.

The falsifier's numbers first: **241 claims attacked, 13 falsified.**

A previous session started C4 and died on a usage limit. Nothing was committed.

---

## Orientation findings (mine, measured this session, before any agent reported)

### F1. The brief is wrong about Agent 2: it produced 342 lines, not nothing

`git diff -- website/scripts/generate-console-data.mjs` shows **342 uncommitted,
unstaged insertions** implementing the data-layer half of the Agent 2 mandate:

- `affects: [...]` on every `KNOWN_DRIFT` entry, one record per method, carrying a
  per-method `caveat`, an optional `wrongText` (the exact false published prose) and
  an optional `correction`.
- `attachExceptions()` (line 1190), which hangs `caveats` and `corrections` on each
  method and fails the build when a `wrongText` is no longer published, appears in two
  blocks, or appears more than once.
- `ROLE_PAIRS`, `roleOfMethodName`, `reinterpretationsIn`, `reinterpretNote`,
  `rewriteForeignFields`, `assertInheritedMeaningIsTrue` (lines 653 to 790): alias
  resolution that rewrites role words from the document's own reinterpretation clause
  and refuses any inherited meaning contradicting the method it landed on.

The `getBalance` caveat in that work is already correct: `"nonce here is the committed
one"`, not the inverted `"not the account nonce"`.

**But it does not reach the page, and three checks prove it:**

1. `src/data/console-data.generated.json` contains zero occurrences of `caveats`,
   `corrections`, `reinterpreted` or `notesFrom`. The generator has not been re-run.
2. `scripts/generate-console-html.mjs` never references `caveats` or `corrections`.
3. `console.html` contains zero occurrences of either.

So the work is roughly half done: correct at the data layer, unrun, and unwired to the
renderer. It must be judged rather than redone. Agent 2 was told this explicitly.

### F2. Error 2's exact mechanism, and a fifth error of the same class

`generate-console-html.mjs:415-429`, `methodNotes()`, attaches index badges by
regex-scanning each exception's **prose**:

```js
for (const e of d.drift.value.knownExceptions) {
  const mentioned = [...String(e.summary + " " + e.why).matchAll(/novai_[A-Za-z]+/g)].map((m) => m[0]);
  for (const name of new Set(mentioned)) add(name, CAVEAT_LABELS.get(e.id) ?? e.id);
}
```

The `getnonce-documented-as-interchangeable` summary names both methods, so both rows
get `"not the account nonce"`. Confirmed in the rendered index at lines 199 and 201.

**The same mechanism fails in the opposite direction, and this is a fifth error the
brief does not list.** `error-code-32014-undocumented` is entirely about
`novai_submitTransaction`, but neither its `summary` nor its `why` contains the literal
string `novai_submitTransaction`. So `methodNotes()` attaches it to nothing and the
`novai_submitTransaction` index row is **empty** (rendered line 205), despite being the
one exception the generator itself calls client-breaking.

Over-attaches and under-attaches. A prose regex is not a per-method declaration, and
this is exactly why the fix has to be `m.caveats` from `affects`, not a better regex.

### F3. The seven alias-resolved methods, measured from the generated data

| method | params from | result from | errors from |
|---|---|---|---|
| `novai_getBlockByHeight` | - | `novai_getLatestBlock` | - |
| `novai_getBlockByHash` | - | `novai_getLatestBlock` | - |
| `novai_getNonce` | `novai_getBalance` | - | `novai_getBalance` |
| `novai_listVkRegistrations` | - | - | `novai_getVkRegistration` |
| `novai_getActiveSla` | - | `novai_getSlaAgreement` | `novai_getSlaAgreement` |
| `novai_listSlasBySeller` | `novai_listSlasByBuyer` | - | `novai_listSlasByBuyer` |
| `novai_listChannelsByPartyB` | `novai_listChannelsByPartyA` | - | `novai_listChannelsByPartyA` |

The role-word heuristic can only ever see the last two. The other five are invisible to
it, which is why the hand audit is mandatory.

### F4. The snapshot matches the working tree

`src/data/console-data.generated.json` and `public/openrpc.json` are both stamped
29 Aug 15:09 local, and the rendered snapshot's provenance line reads
`2026-08-29 13:09:38 UTC`. Same run. The frozen page is the current tree, so the
falsifier's findings apply to code that is still there.

### F5. A SIXTH error, and it may be the most consequential of all: 15 of 29 methods publish a result type the page never defines

Measured from the generated data: **15 of the 29 methods** render a result shape whose
payload is a bare type name with no definition anywhere on the page.

| type | methods publishing it |
|---|---|
| `SlaAgreement` | `getSlaAgreement`, `getActiveSla`, `listSlasByBuyer`, `listSlasBySeller` |
| `OracleAnchor` | `getOracleAnchor`, `getOracleAnchorsByEntity`, `getOracleAnchorsByTag` |
| `PaymentChannel` | `getPaymentChannel`, `listChannelsByPartyA`, `listChannelsByPartyB` |
| `VkRegistration` | `getVkRegistration`, `listVkRegistrations` |
| `PaymentRecord` | `getPaymentsByEntity` |
| `ServiceDescriptor` | `getServiceDescriptorsByCategory` |

The page prints the note **"Record shape declared once for `<category>` and shared by
every method in it"** exactly **17 times** (rendered text, grep count). Declared where?
Not on this page. `docs/RPC_REFERENCE.md` defines all six types in `jsonc` fences at
lines 717, 806, 892, 997, 1171 and 1428, and **none of the six reaches the console.**

Proof by field name, against the rendered snapshot: `basis_points` appears **0 times**
and `anchor_data_hash_equals` appears **0 times**, though both are fields of
`PaymentRecord` as the document defines it at `docs/RPC_REFERENCE.md:717-745`.

This is a different class from the four known errors. Nothing here is a false
statement. It is a promise the page makes about itself, seventeen times, and does not
keep. A developer building a payment client cannot learn from the console what a
payment record contains, that `amount` is a decimal string rather than a number, or
which four `condition.kind` values exist. They have to leave and read the document the
console exists to replace, which is precisely the bar Gate C4 is measured against.

The fix is additive rather than corrective: render the six category record fences once
per category, and make the bare type name in every result envelope a link to it.

### F6. The certificate, measured by me today rather than inferred

```
subject=CN=rpc.novai.network
issuer=C=US, O=Let's Encrypt, CN=YE1
notBefore=Aug  1 04:49:38 2026 GMT
notAfter=Oct 30 04:49:37 2026 GMT
```

61 days out. `NEEDS-OPERATOR.md:281-288` carries "expires 2026-08-30" plus an appended
"**Update 2026-08-29: that is tomorrow.**" Both lines are wrong and the second is the
worse kind of wrong, because it is an inference from a stale document dressed as an
observation. Both must be replaced with the measurement and the command that produced
it.

### F7. The tokeniser question is settled by the content, not by taste

Independently reproduced the block inventory from the snapshot HTML: **84 `<pre>`
blocks, 51 JSON, 31 shell, 1 Python, 1 Rust.** (Median 248 characters on decoded text;
the previously recorded 348 was measured on a different basis. The distribution matters,
the exact median does not.) Instrument check: the file contains 84 `<pre` and 84
`</pre>`, so the blocks do not nest and the extraction is sound.

Two measurements decide the hand-rolled-versus-dependency question:

**1. `JSON.parse` fails on 29 of the 51 JSON blocks.** Not a defect. The fences are a
domain schema notation with three constructs no JSON grammar knows:

  - type placeholders, `<u64>` / `<hex32>`, in 15 blocks
  - bare record-type references as values, `{ "channels": [PaymentChannel, ...] }`, in 14
  - `//` line comments

Shiki's `json` grammar rejects all three and `jsonc` rejects two of the three. A real
grammar is therefore **less** accurate here than a hand-written one, not more. I have
not run Shiki to observe exactly how it degrades, and I am not claiming to have; what is
measured is that 29 of 51 blocks are not the language it would be asked to parse.

**2. The shell surface is one command and four flags.** Across all 31 shell blocks the
only leading word is `curl` and the only flags are `-s -X -H -d`. There is nothing here
for a general highlighter to earn its dependency on.

So: hand-rolled build-time tokeniser, and the argument is not dependency hygiene. It is
that a tokeniser I control can give `<u64>` and `PaymentRecord` their own token classes,
which lets colour say **"this is a slot you fill"** and **"this is a type defined over
there"**. The second one becomes a hyperlink, which is how F5 gets fixed. No off-the-shelf
highlighter can do either, because the notation is ours. Still going to the operator as
a question, but with a recommendation and the evidence behind it.

### F8. The region-count gate under a page split, designed rather than discovered later

Today, `findRegions()` at `generate-console-html.mjs:831-871` asserts, against one file:
markers are paired and never nested; found region ids set-equal `REGIONS.map(id)`; the
counts are equal; no id appears twice. `--check` then re-renders and byte-compares.

A naive port to eight pages makes every one of those checks per-file, and the global
guarantee that **every region is rendered somewhere** vanishes without anything going
red. The replacement:

- Declare `PAGES`: page file to its ordered region ids. `REGIONS` stays the single
  source of renderers.
- Per file: pairing, nesting and no duplicates, exactly as now, plus found ids
  set-equal the ids declared for **that** page, so a region on the wrong page fails.
- Globally: the multiset union of found ids across all pages equals `REGIONS.map(id)`
  with **multiplicity exactly one**. That is the check the naive port loses, and it
  catches both a region rendered nowhere and a region rendered twice.
- Failure probes, per rule 9: add a region to `REGIONS` and assign it to no page; assign
  one region to two pages; move a region to the wrong page. Each must fail, and the
  probe must be shown to have been seen.

### F9. The searchable surface is 88 symbols, which makes the findability problem small

Measured from the rendered snapshot:

- **40** `SCREAMING_SNAKE` constants (`MAX_SIGNAL_QUERY_RANGE`, `MIN_FEE_REGISTER_AI_ENTITY`,
  `PRUNE_RETAIN_BLOCKS`, `MARKETPLACE_FEE_BPS`, ...)
- **13** distinct JSON-RPC error codes, `-32000` through `-32700`
- **29** method names
- **6** record type names

Roughly **88 named things** on the entire surface. A generated symbol index page with 88
rows is trivial to build, ships zero JavaScript, and answers "which page holds this
name" completely. It does not answer free-text search, which is what a generated
all-in-one page answers. The two together cover the whole loss, and neither needs a
byte of JS. Waiting on Agent 1's evidence before committing to this.

One correction to the brief: `MIN_ACCOUNT_BALANCE`, its worked example of the
findability problem, appears **zero times** on the page. It is not a constant of this
system. The argument is unaffected; `MAX_SIGNAL_QUERY_RANGE` makes it identically. Noted
only because repeating a constant that does not exist is the failure mode this gate is
about.

### F10. A negative result worth recording: no phantom method reaches the page

`website/HANDOFF.md` warns that `crates/node/src/rpc.rs` carries three doc-comment
references to `novai_getStatus`, a method that does not exist, so a loose regex injects
a phantom. Checked: the rendered page carries 30 distinct `novai_*` strings, of which 29
are the real methods and the thirtieth is `novai_sdk`, the Python package name. No real
method is missing from the page and no phantom is on it. The hardening held.

### F12. The page split has a production-only failure mode already latent in the config

`tailwind.config.ts:7-13` names **one** HTML file:

```js
content: ["./pages/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}", "./app/**/*.{ts,tsx}", "./src/**/*.{ts,tsx}", "./console.html"],
```

`vite.config.ts:33-36` names two entries, `main` and `console`. Splitting into eight
pages means eight new HTML files, and any one of them not covered by a content glob has
**every utility class on it purged**. Dev looks perfect, because dev does not purge.
Production ships an unstyled page. This is the exact failure the C3 work hit and
diagnosed empirically.

Two changes, and the second is the one that matters:

1. Widen the glob to cover every console page.
2. **Gate it.** A glob is not a guarantee; the next page added in six months is the one
   that gets missed. Two assertions: every HTML file in `rollupOptions.input` is matched
   by at least one Tailwind content glob, and, after a build, a probe class known to
   appear only in each page's generated markup is present in the built CSS. The second
   is the automated form of the manual `list-disc` / `pl-5` check the C3 work ran by
   hand, and it is the only one of the two that measures the actual output rather than
   the config's intent.

### F13. The current layout, measured, which is what "use the width" is about

`console.html:86-105`: a two-column shell, a sticky left rail (`hidden lg:block`, so it
disappears below 1024px) and `<main class="min-w-0 px-4 py-8">`. `<main>` carries no
max-width, so it stretches to the viewport, while prose inside it is capped at
`max-w-[70ch]` (`console.css:46`) and code blocks are `w-max max-w-full`
(`console.css:59,64`). At 1440px that leaves several hundred pixels of empty right
margin next to every paragraph, while code sits under the prose rather than beside it.

That is the complaint, precisely located. The Stripe pattern (prose left, code right)
fits: at 1440px, minus a ~240px rail, `<main>` has roughly 1200px, which is two ~560px
panes. At 2560px it needs an explicit container cap or the panes become unreadable. The
settled rule that tables and KV grids size to content, never `w-full`, survives this
unchanged: it is the prose-and-code relationship that changes, not the data elements.

### F14. Exactly one chart earns its place, and the fee ladder is it

Read the four candidate datasets out of `console-data.generated.json` rather than
guessing which would chart well.

**`fees`, 11 transaction types, verified.** Six distinct values across eleven types:
`transfer` and `creditAiEntity` at 100; the three memory-object operations and
`executeProposal` at 500; `signalCommitment` at 1000; `submitProposal` at 2000;
`registerAiEntity`, `registerAiEntityWithKey` and `entityUpgrade` at 5000. All eleven
cite `crates/execution/src/lib.rs` lines 12325 to 12349. **5000 / 100 = 50x**, so the
brief's "registering an entity costs 50x a transfer" is exact.

This one charts because the fact is a **ratio**, and a table of eleven numbers makes the
reader do the division. A sorted bar also surfaces something the table buries: the three
most expensive operations are all entity-lifecycle, and the ties are informative rather
than noise (every memory-object operation costs the same). Static SVG generated at build
time, zero JavaScript.

**The other three do not earn it.** `sdkCoverage` is an 11-by-3 matrix that is already
a table and reads better as one. `signalPayloads` is a base of 66 bytes with tails on a
minority of the 23 types, so a bar chart is mostly a flat line saying "these are all the
same". `errorCodes` is 13 values in three classes, which a table with a class column
communicates completely. Proposing **one** chart, not four.

### F11. Scope note for the operator

`website/HANDOFF.md` defines Gate C4 as **live panels** (network status and verify panel
as React islands, plus a new RPC endpoint resolver module). This session's brief defines
Gate C4 as **data correction, page split, colour and charts**. Those are different
workstreams. The brief mentions a live block-cadence sparkline "belongs with the
islands", so the islands work still exists somewhere. Flagged as a numbering question,
not resolved unilaterally.

---

## Phase 1 agent reports

Written here as they arrive.

### Agent 1: RESEARCH

**Method, stated up front by the agent and load-bearing for how much weight to give it:**
no site was rendered in a browser. Every finding is raw server HTML, CSS custom-property
names and values, sitemap enumeration and `llms.txt` / `.md` endpoints fetched with
`curl`. For colour that is **stronger** than a screenshot, because it read semantic
variable names (`--sh-token-string`, `--fd-toc-width`) rather than inferring intent from
pixels. For layout and interaction it is weaker, and it says so.

#### Q1. Page architecture

**Only two sites put every method on one page, and only one of them works.**

- **ethereum.org is the clean success**, and it is the direct precedent for keeping 29
  methods together: 38 methods and 86 code blocks on one server-rendered page with 38
  stable `#eth-*` anchors and a 38-entry on-this-page rail. Our 29 is **below proven
  scale**.
- **Sui is the cautionary case.** `/sui-api-ref` is a 22.5 KB JavaScript shell whose
  server HTML contains the literal string `Loading OpenRPC...`, **zero** `href="#"`
  anchors and **zero** method names. `grep -c 'suix_getBalance'` returns 0 against both
  the page and its own `llms-full.txt`. Sui's entire method reference is invisible to
  Ctrl-F, to deep links, to crawlers and to agents.

**NEAR is the exact shape proposed here:** its whole RPC API is **8 topical pages**
(`introduction`, `transactions`, `block-chunk`, `contracts`, `gas`, `protocol`,
`providers`, `batching`). Not one page per method, not one page for everything. Whole
site: 181 URLs.

**Alchemy is the reductio for fragmentation:** 5,290 URLs, 4,157 under `/chains`,
generated as roughly 98 method pages by 42 chains, so `eth-get-balance` exists 42 times as
near-identical prose. Nothing is browsable; the reader is wholly dependent on search.

#### Q2. Search and findability, the deciding question

**Finding 1: not one of the eleven sites has search that works with JavaScript disabled.**
Tested directly: `docs.sui.io/search?q=` returns 22,834 bytes with no results in the body;
`quicknode.com/docs/search?q=` returns 14,966 bytes with no results; `solana.com/docs/search?q=`
**404s**; `docs.near.org/search?q=` **404s**. Algolia DocSearch, Mintlify's built-in and
Fumadocs/Orama are all client-side only. Cost of the industry-standard choice:
`@docsearch/js@3` UMD is **132,960 bytes** plus **13,924 bytes** of CSS, plus a network
round-trip per keystroke, plus a hosted-crawler dependency.

**Finding 2, and this is the answer: the entire industry has already converged on a
zero-JavaScript, build-time flat-text corpus.** Every site probed ships one.

| site | `llms.txt` | `llms-full.txt` |
|---|---|---|
| Alchemy | 472 B | **11,328,533 B** |
| Solana | 95,456 B | **4,638,517 B** |
| Helius | 17,099 B | 2,544,059 B |
| Aptos | 13,199 B | 2,285,983 B |
| NEAR | 33,143 B | 1,043,756 B |
| Base | 92,611 B | 95,547 B |
| QuickNode | 28,860 B | 91,008 B |
| Sui | 42,896 B | 77,809 B (API ref absent) |
| Stripe | 90,052 B | 404, but every page has a `.md` twin |
| ethereum.org | 12,985 B | JSON-RPC absent |

`llms-full.txt` **is** the one-keystroke Ctrl-F surface, built at build time, shipping zero
JavaScript, working with JS disabled.

**Finding 3: Solana has the most complete findability system in the sample and almost none
of it is JavaScript.** Verified: `/docs/rpc/http/getaccountinfo.md` returns 200 and 13,020
bytes; the same URL with `Accept: text/markdown` returns the same 13,020 bytes as
`text/markdown`. Stripe's variant is the terminal: a `.md` twin on every page
(`/api/charges/create.md`, 8,975 bytes) plus **33 `@media print` rules**, a real print
stylesheet, which is a second zero-JS whole-surface view.

**Ranked answer, cheapest first:** (1) generated `/all.txt`; (2) **generated `/all.html`,
which is `llms-full.txt` with anchors** and is strictly better; (3) per-page `.md` twins;
(4) a generated symbol index, which **no site in the sample does for constants**; (5) a
plain GET form to an external engine; (6) client-side index (Pagefind); (7) Algolia, the
most expensive and the one most of the sample chose.

For a roughly 3,000-line console, `/all` is about 150 KB, against Solana's 4.64 MB and
Alchemy's 11.33 MB, both of which browsers Ctrl-F without complaint.

#### Q3. Navigation

Global nav and sidebar persist everywhere. **Two clicks to a method is actually one**: the
sidebar carries the full method list, not a category that expands. Solana's server HTML
contains all 52 method links on every method page; Helius's sidebar carries 55
`method-nav-pill` elements. **Solana deliberately turns the on-this-page rail OFF on
method pages** (`hideTableOfContents: true`) because the two-pane code panel has taken the
third column. Breadcrumbs are present on Sui, Solana and ethereum.org and **absent on
Stripe**, by far the deepest site in the sample. Sidebar scroll preservation could not be
verified and the agent declined to claim it.

#### Q4. What colour is DOING

**Stripe: 27 token classes collapse to 5 colours. The single most useful finding in the
report.** Stripe declares a full semantic set (`--sh-token-attribute`, `-class`,
`-function`, `-keyword`, `-string`, `-variable`, and 22 more) that resolves to five values:
foreground `#c9ced8`; muted `#768193` for **comments and punctuation together**; keyword
`#2b9df6`; string `#3eae20`; number, char and escape `#f27400`. **Stripe deliberately does
not colour function names, class names, variables, tags or attributes.** The best technical
reference on the internet distinguishes four things: keywords, strings, numbers, comments.

**ethereum.org: 14 colours, but the dominant treatment is alpha, not hue.** Its most-applied
token colour on the JSON-RPC page is `#C98A7D77`, used **1,720 times**, at 47% alpha, for
punctuation. Foreground is `#DBD7CAEE` (93%) and there is a `#758575DD` (87%) tier. Its
primary encoding for "this token is structural noise" is **dimming**. Directly transferable
to a monochrome console at zero colour budget.

**Helius: colour marks the HTTP verb, in the nav.** `bg-blue-400/20 text-blue-400` tinted
pills, once in the header and **55 times in the sidebar**. A 20% alpha tint with matching
foreground survives a no-gradient, no-glow rule cleanly.

**Solana: deprecation is marked with text, not colour** (`Deprecated. Legacy encoding
retained for backwards compatibility... Use base64 instead.`), which is correct, because a
deprecation needs a replacement named and a colour cannot carry one.

#### Q5. Layout width

Stripe's `ApiSectionGrid`, measured from served CSS: `grid-template-columns: repeat(2,
minmax(0, 1fr))`, `column-gap: min(3rem, 5vw)`, `row-gap: 2.5vw`, `align-items: start`,
collapsing to `1fr` at **1000px**. Page cap `max-width: 110rem` = **1760px**. Named width
variables include 768px and 490px. ethereum.org: three sticky regions, container
`max-w-screen-2xl` = 1536px, prose `max-w-3xl` = 768px, breakpoint 767/768px. Solana:
`--fd-sidebar-width: 268px`, `--fd-toc-width: 268px`, containers `max-w-[1440px]`.

**At 2560px nobody lets it grow.** Stripe caps at 1760px, ethereum.org at 1536px, Solana at
1440px. All three centre with large gutters. The convergence is unanimous.

#### Q6. Charts

**Essentially none of them do this, and the finding is emphatic.** Alchemy's
`compute-unit-costs` page, the single most chartable page in the sample, contains **44
`<table>` elements and zero charts**. Greps for recharts, chart.js, plotly, nivo, visx,
mermaid and `<canvas>` across Solana, Alchemy, Base and Helius return **zero hits**. SVG
counts are icons and logos. Status visuals exist only on separate third-party Statuspage
instances, never inside the docs. Every comparison, cost matrix and coverage grid in the
best technical references on the internet is an HTML table. The zero-JS way to make a table
read as a visual is to tint cells by value, which is a class on a `<td>`.

#### What it could not determine

It never rendered a site. Sidebar scroll preservation unverified. Sui's design unknowable
behind its JS shell. Stripe's `/api` landing is JS-hydrated, so all Stripe findings come
from a per-method page. Solana and Sui token colours are client-rendered, so its colour
counts are firm only for Stripe (5), ethereum.org (14) and Helius (7 dark, 6 light).
Mintlify pixel widths are inferred from Tailwind v4 spacing, not computed. The DocSearch
figure is unminified UMD, not transferred bytes. Tron, Aptos and Base were verified for
platform and search only.

### Agent 2: DATA CORRECTION

It confirmed all three facts it was given, and then found the one that changes the plan.

#### The blocking defect: the uncommitted work can never reach the page, even if rerun

`generate-console-data.mjs:2469-2482` whitelists the emitted per-method keys:
`name, category, brief, description, params, result, errors, curl, sampleResponse, exampleNote`.
`attachExceptions()` hangs `m.caveats` and `m.corrections` onto the in-memory objects and
**`payloadFor` silently discards both**. Re-running the generator today would still produce
a JSON with no caveats and no corrections. The work is not merely unconsumed by the
renderer; it is **unserialised**, and that is invisible from the diff. The falsifier reached
the identical conclusion by a different route, confirming `"caveats" in method` is false for
every method in the shipped JSON. Two independent agents, one answer.

By contrast `reinterpreted`, `notesFrom` and `errors.rewrites` **would** survive, because
they live inside `m.params` / `m.errors`, which are emitted wholesale.

It verified `--check` writes nothing by reading `main()` at `:2587`, the check branch at
`:2653-2658` (two `checkFile()` calls then a bare `return`), and confirming the only write
site `writeAtomic` at `:2698-2701` is reached at `:2661` **after** that return. Then ran it
and re-checked mtimes. That is the instrument-first discipline this project requires.

#### Task A: all seven alias-resolved methods, by hand

| method | block | verdict |
|---|---|---|
| `getBlockByHeight` | result | **WRONG**, null case dropped |
| `getBlockByHash` | result | **WRONG**, null case dropped |
| `getNonce` | params | correct |
| `getNonce` | errors | **WRONG**, `-32002` cannot occur |
| `listVkRegistrations` | errors | **WRONG**, `` `id` `` is not a field of this method |
| `getActiveSla` | result + errors | correct, and provably |
| `listSlasBySeller` | params | **WRONG**, "buyer entity id" |
| `listSlasBySeller` | errors | correct |
| `listChannelsByPartyB` | params | **WRONG**, "party A entity id" |
| `listChannelsByPartyB` | errors | correct |

**Five of the seven carry at least one wrong inherited block. The heuristic saw two.** The
hand audit was justified.

Highlights:

- **`getNonce` errors, the most important new one.** `handle_get_nonce`
  (`crates/node/src/rpc.rs:2082-2107`) has signature `(request, nonce_provider)` and is
  dispatched at `:1385` as `handle_get_nonce(&rpc_request, &nonce)`. **It is never handed
  the database.** Its whole body is a hex parse plus `nonce_provider.expected_nonce()`. It
  cannot emit `-32002`; the row was inherited from `handle_get_balance`, which genuinely
  reads state at `:2720-2723`.
- **`listVkRegistrations` contradicts itself inside one method block:** rendered 1350 shows
  the params row `entity_id`, rendered 1370 shows the error clause naming `id`, rendered
  1381 shows the curl passing `entity_id`. `ListVkRegistrationsParams`
  (`rpc.rs:598-601`) declares only `entity_id`, validated at `:2371`.
- **`listSlasBySeller`'s own curl already contradicted its own table.** Rendered 1596 passes
  `5b5b...5b5b`, the byte pattern the reference uses for the **seller** argument of
  `getActiveSla`, while every buyer value is `44a2...2ac1`. The example was right and the
  table was wrong, two hundred pixels apart. Same for `listChannelsByPartyB` at rendered
  1776. The Rust is explicit that the field is role-neutral and the method decides the
  role: `ListSlasParams` (`rpc.rs:708-711`) and `ListChannelsParams` (`rpc.rs:829-832`)
  both carry a rustdoc saying "buyer or seller, depending on the RPC method".
- **`getActiveSla` is what a safe alias looks like:** `handle_get_active_sla` (`:2414`) is
  declared returning the **same Rust type** as `handle_get_sla_agreement` (`:2386`), so
  field-for-field identity is guaranteed by the type system rather than by inspection.

#### Holes in the proposed gate, which is the most valuable part of this report

| WRONG | caught by `assertInheritedMeaningIsTrue`? |
|---|---|
| `listSlasBySeller` / `listChannelsByPartyB` roles | yes, and repaired upstream |
| `listVkRegistrations` `` `id` `` | silently **repaired**, not reported. See W-a |
| **`getNonce` `-32002 DB read failure`** | **NO. Structural blind spot** |
| **`getBlockByHeight` / `ByHash` lost null case** | **NO. Structural blind spot** |

The `-32002` miss is the damning one: it is exactly the failure mode the gate advertises,
an inherited error clause false of the method it landed on, and the gate cannot see it
because the clause carries no role word (check (a) never runs) and no backticked identifier
(check (b) matches nothing). It is a plain-English claim about a code path that does not
exist.

The null-case miss is the deeper one, and the agent stated the general principle:
**"a gate that reads what was copied cannot detect what was not copied."** The gate inspects
`params[].notes`, `errors[].when`, `errors.text` and `result.note`. It never inspects
`nullable`, and the alias resolver at `:2135` deliberately overwrites `nullable` with the
alias block's own value.

**Its own recommendation, which I endorse and rank above the role heuristic: cross-check
every method's curl against its own params table.** Both role bugs had a curl that already
disagreed with the published note, and unlike the role vocabulary this check generalises to
methods with no role word in their name, which is the whole limitation of the heuristic.

#### Wrong or dangerous in the uncommitted work

- **W-a. `rewriteForeignFields` silently edits a quotation of the reference with no on-page
  provenance.** It rewrites `` `id` `` to `` `entity_id` `` for `listVkRegistrations`. The
  substitution happens to be right, but the guard "exactly one field of the same type" is
  **always** satisfied for any single-parameter method: it is unique-by-arity, not forced by
  shape, and the comment claiming otherwise is wrong. The log lands in `m.errors.rewrites`,
  which the renderer never reads, so the page would publish a doctored quotation of
  `docs/RPC_REFERENCE.md` with no indication the console changed the document's words. On a
  page whose thesis is "here is where the source is wrong", that is a new dishonesty.
  **Recommendation: keep the detection, delete the automatic rewrite,** and let the
  correction machinery be the honest home for it.
- **W-b.** The check-(b) field sets are mismatched: the loop is guarded on
  `m.errors?.resolvedFrom` but iterates params- and result-sourced items too, against the
  errors source's field set. A method with `params.resolvedFrom` and no `errors.resolvedFrom`
  gets **no foreign-field check at all**.
- **W-c.** `reinterpretNote`'s fallback returns `interpreted as ${role}`, discarding every
  non-role fact in the note (types, bounds, encoding). Should `fail()` rather than truncate.
- **W-d.** Nothing asserts the inverse direction: that every rendered caveat came from an
  `affects` record. That inverse is precisely what makes error 2 unreintroducible.
- **W-e.** The `affects` records are not gated against the exception's own measured facts;
  two independent readings of one sentence must agree and nothing enforces it.

#### Task B: strike through, do not suppress. Its argument, which I accept

Three reasons, in its order of weight. (1) The false sentence is still published in
`docs/RPC_REFERENCE.md`, which every method row links to, and the console cannot retract it;
a reader who sees the console silently omit a sentence the reference contains cannot tell
curation from staleness from error. (2) The known-gaps table becomes **checkable** rather
than merely assertable: the claim "five discrepancies are carried" is today unfalsifiable
from the page, and striking each at its own site turns the list into an index of five
visible corrections. (3) The confusion cost is bounded (one reading tax on five sites out of
29) and is answered by **placement**, putting the correction on the next line with a pill so
the eye lands on the true statement; the alternative's cost is an invisible permanent
divergence that a future maintainer eventually "restores".

It specified the markup using only existing vocabulary and **no new CSS class**, on the
stated ground that new component CSS needs a design-rules test and buys nothing that
`console-note` plus `console-pill` do not already say. `<del>` carries the semantics for
assistive tech. Its `richStruck()` splits the **raw markdown** and enriches three fragments
separately rather than string-replacing inside enriched HTML, so no split can land inside an
entity or an emitted `<code>` tag, and it fails on unbalanced backticks in `wrongText`.

#### Task C: keep the entry, strip the payload

Its finding that decides it: `agreedMethodCount` is `c.docNames.size`
(`generate-console-data.mjs:2457`), which counts `###` headings in the reference, **not**
rendered entries, and the four-way gate compares name sets. **So the number 29 is
doc-derived and survives any rendering choice.** Hiding the entry entirely would leave a
`29 methods, agreed` tile above a 28-row table, break the existing "renders every method
with an anchor and a source link" test, break deep links to `#novai_faucet`, and make the
console the only one of four cross-checked sources that omits a method the gate says all
four agree on.

What the page publishes today at rendered 2151-2214 is a params table, a result fence naming
the mint amount, **a runnable curl that mints 10,000,000 tokens against `$URL`**, and a
sample response with a real txid. Option (ii) withholds all of that, keeps the anchor and
source link, and keeps both faucet caveats and corrections visible.

One count breaks and it named it: the "labels reference sample responses" test asserts
`labels === withSamples.length`, and `withSamples` counts 19 methods with a non-null
`sampleResponse` including faucet. Suppressing faucet's example makes it 18. It must become
`filter(m => m.sampleResponse && !m.withheld)`.

It flagged the judgement call honestly: it reads "a visible faucet" (`HANDOFF.md:349`) as a
**feature** exclusion, since it sits with repo statistics, block browsing and address
lookup, and `HANDOFF.md:375` glosses it as "Build no faucet UI". If the operator reads the
ruling as covering the name itself, fall back to hiding the entry and pay the four costs.

#### Task D sweep: 25 hits, several new

Beyond the alias and badge findings already covered:

- **D16, new: the false gating claim is in the index, where no correction can reach it.**
  The faucet brief at rendered 243 reads `Mint test tokens (dev mode only)`. That
  parenthetical is the same falsehood as the description, republished in a cell no
  correction mechanism touches.
- **D17, latent:** `getBlockByHeight`'s result note carries "this should be unreachable
  given the validation", which rendered 2903 measures as false. Not published today only
  because `renderResult` never prints `m.result.note`. **Any fix that starts rendering
  result notes publishes a falsehood the page already refutes.** That is a direct trap for
  the F5 record-shape work.
- **D18 to D25, hand-typed numbers that duplicate derived ones.** `console.html:114` types
  "Five known discrepancies" fifteen lines above the derived `5 carried exceptions`, and
  **it is outside every sentinel region, so the marker gate does not govern it**.
  `console.html:119` types 23 and 16 the same way. `generate-console-html.mjs:133` types
  "Thirteen", "four" and "six" as words. Three further hardcoded `29`s and a hardcoded
  `13`. All agree today; nothing enforces any of them.
- **D20/D21, dead duplicates:** `PROSE.intro` and `PROSE.differentiator`
  (`generate-console-html.mjs:96-105`) are byte-for-byte the sentences hardcoded in the
  HTML and are **referenced by no renderer**. `PROSE.rpc` is also dead, and it asserts
  "the notes column names the caveat that applies to a method before you open it", a
  sentence vouching for the very column that shipped inverted. Two copies of a claim exist,
  one live in HTML and one dead in the generator, free to diverge.

#### What it did not verify

It did not run the HTML generator's `--check` or the vitest suite (the suite writes to
tmpdir), so its claim that the sample-response test fails under option (ii) is derived by
reading the assertion and counting, not by observing a red test. It did not re-run the data
generator in write mode, so the `payloadFor` conclusion is read off the whitelist rather
than observed. It did not call the endpoint. It did not open `expected_nonce`, which lives
outside `rpc.rs`, so the `-32002` finding rests on the signature, the dispatch arm and the
body. It read `docs/RPC_REFERENCE.md` only at cited lines and `HANDOFF.md` only at 330-400.

### Agent 4: THE FALSIFIER

**CLAIMS ATTACKED: 241. CLAIMS FALSIFIED: 13.**

#### My judgement on whether the attack was real: yes, and by the measure that was set

It was not told the four known errors. It found three of them unaided, ranked the worst
of them first, found the fifth error I had found myself, and added five substantial
findings nobody had. Ranked by what it independently recovered:

| known error | falsifier result |
|---|---|
| 2. `getBalance` badge backwards | **found, ranked #1** |
| 1. `getNonce` self-contradiction | **found, #2 and contradiction #1** |
| 3. alias role meanings | **found both instances, #5 and #6** |
| 4. faucet fully documented | found its two factual errors (#3, #4). Did not flag that the faucet should be hidden, which is a ruling it was never given, so this is not a miss |
| 5. `submitTransaction` empty badge (mine) | **found independently, #12** |
| 6. record shapes never defined (mine) | **not found.** The one real gap in its coverage |

It verified its own instruments before trusting them: it poisoned two entries in its
citation checker and confirmed it caught exactly those two; it reproduced `methodNotes`
against the shipped JSON and matched the rendered page before trusting its
`submitTransaction` result; it confirmed the em-dash finding with `od -c` rather than by
eye. It hit the `grep` exit-1 short-circuit trap live (two `grep --include` calls died on
zsh globbing with no matches and exit 1) and re-ran them quoted rather than reading the
silence as clean. And it recorded a near-miss where it was wrong and the page was right
(`getSignalsByIssuer` takes `issuer`, not `entity_id`).

#### It missed the sixth error, and I am stating that unsoftened

The falsifier landed before the operator's instruction arrived, so it was told nothing
about F5 and the test was clean. **It did not find the type-definition gap.** Fifteen of
29 methods publish a result payload naming a type the page never defines, the page prints
"Record shape declared once for `<category>` and shared by every method in it" seventeen
times without ever rendering a shape, and `basis_points` and `anchor_data_hash_equals`
appear zero times on a page that claims to be everything a developer needs. It read all
2,995 lines and did not raise it. That is the largest single hole in a 241-claim pass, and
it is a hole in the direction of the page's core promise rather than in some corner.

**My judgement on the operator's conclusion, which is the one thing here I do not simply
accept.** The rest of the run does not read as theatre: it verified its instruments before
trusting them, hit the `grep` exit-1 trap live and re-ran quoted rather than reading
silence as clean, found five substantial defects nobody had, two of which I have since
reproduced byte for byte myself, and recorded a case where it was wrong and the page was
right. A pass that produces findings 8, 9 and 11 is not a pass that was going through the
motions.

The miss has a cause, and the cause is partly mine. I wrote its protocol as
**"for every factual claim, state what evidence would prove it FALSE."** F5 is not a false
claim. It is an absence, and an absence has no falsifying evidence to describe, so a
falsification protocol walks past it. The proof that this is the mechanism rather than an
excuse is that Agent 2, whose mandate was to sweep for **classes** rather than to falsify
**claims**, found the structurally identical absence unaided (the dropped `nullable` on the
two block methods) and wrote the general principle down: *a gate that reads what was copied
cannot detect what was not copied.* Same blind spot, and the agent whose mandate pointed at
it found it.

Against my own defence: the falsifier did find three absences (the two missing
`can answer null` badges and the missing `submitTransaction` badge), so it was not
constitutionally blind to missing things. It found missing **badges** and not missing
**definitions**. So the mandate hole is a partial explanation, not a full one.

**Verdict I will stand behind: the attack was real and incomplete, not unreal.** The
correction is to the mandate, not to the roster. When Agent 4 re-runs in Phase 3 it gets an
added instruction with the same standing as the falsification protocol: *for every promise
the page makes about its own content, find the content and confirm it is there; a
cross-reference to something the page does not contain is a defect of the same rank as a
false statement.* If the operator wants the harder line recorded anyway, it is recorded
above in the first paragraph, unhedged and in its own words.

#### The 13 falsifications, ranked as it ranked them

1. **`getBalance` pill inverts its own meaning.** Rendered 193. `crates/node/src/rpc.rs:2707-2727`
   returns `nonce: account.nonce`, the committed account nonce. The generator's own data
   says the caveat should be "nonce here is the committed one"
   (`generate-console-data.mjs:1099-1103`); that string never reaches the page.
2. **`getNonce` publishes prose the build formally flagged as client-breaking.** Rendered
   484-486. The correction is computed at `generate-console-data.mjs:1220` and dropped.
   Zero references to `corrections`, `wrongText` or `strike` in the HTML generator.
3. **Faucet gating published backwards.** Rendered 2154-2156 against
   `crates/node/src/rpc.rs:2990-2992`: the handler prefers `--faucet-key` and falls back
   to dev keys, so it answers on a production node. Consequence: an operator leaves a live
   mint endpoint exposed.
4. **Faucet disabled-path code wrong.** Rendered 2183 says `-32602`;
   `crates/node/src/rpc.rs:3003-3007` returns `-32000`.
5. **`listSlasBySeller` tells you to pass the buyer id.** Rendered 1571 against its own
   description eleven lines above and `docs/RPC_REFERENCE.md:1150`.
6. **`listChannelsByPartyB` tells you to pass party A.** Rendered 1750 against rendered
   1739 and `docs/RPC_REFERENCE.md:1292`.
7. **`getNonce` lists an error it cannot emit.** Rendered 516-517 publishes
   `-32002 DB read failure`. `handle_get_nonce` (`rpc.rs:2082-2107`) never touches `db`;
   grep for `32002` in that range returns 0.
8. **NEW, and client-breaking: the `-32600` trigger is wrong in both cases it names.**
   Rendered 2229 attributes `-32600` to "missing jsonrpc/method". `RpcRequest`
   (`rpc.rs:93-98`) declares all four fields non-optional with no serde default, so a
   missing field fails deserialisation at `:1329` and returns **`-32700`**. `-32600` is
   reachable only from `:1348`, a `jsonrpc` field that is present and wrong. Verified
   live against the endpoint today: missing `method` gives `-32700`, missing `jsonrpc`
   gives `-32700`, `"jsonrpc":"1.0"` gives `-32600`.
9. **NEW, and it is self-inflicted by this project's own house rule: the 503 body string
   does not match the bytes the server sends.** Rendered 2306 publishes
   `Service Unavailable - too many concurrent requests` with an ASCII hyphen.
   `crates/node/src/rpc.rs:1241` emits an **em dash**. Cause:
   `generate-console-data.mjs:94` maps em dash to hyphen unconditionally to satisfy the
   repo-wide dash ban, and the substitution is applied to a literal wire string presented
   in a `<code>` block as the body to match on. Confirmed with `od -c` on both sides.
10. **NEW: `can answer null` is missing on `getBlockByHeight`.** Rendered 189.
11. **NEW: `can answer null` is missing on `getBlockByHash`.** Rendered 191. Proven live
    against the endpoint at tip 5,249,321: tip minus 51,000 and tip minus 55,000 both
    answer `{"result":null}`, height 0 answers null, an unknown hash answers null.
    **Mechanism: the badge tracks markdown punctuation, not behaviour.**
    `generate-console-html.mjs:426` gates on `m.result.nullable`, which
    `generate-console-data.mjs:933` sets from `block.qualifier`, the parenthesis captured
    by `readLabel`. `docs/RPC_REFERENCE.md:88` writes the null case as a parenthetical and
    gets the badge; lines 137 and 181 write it as prose and do not. The one method that
    does carry the badge, `getLatestBlock`, is null only "if no blocks have committed
    yet", which is unreachable on a chain at height 5.2M. So the badge is present exactly
    where it is useless and absent exactly where it matters.
12. **`submitTransaction` is missing the badge the build declared for it.** Rendered 197.
    Reproduced `methodNotes` against the shipped JSON: the `-32014` exception's prose names
    no method, so it attaches to nothing. The only state-mutating method on the surface is
    the one with a blank Notes cell.
13. **NEW: the page's claim about its own generation is false in two ways.** Rendered
    39-40: "cross-checked against four independent sources, so the build fails when they
    disagree." (a) The cross-check compares **method names only**; nothing about params,
    error codes, result shapes or notes is compared, so findings 5, 6, 7 and 8 are content
    it structurally cannot see. (b) `git show HEAD:website/scripts/generate-console-data.mjs
    | grep -c assertInheritedMeaningIsTrue` returns **0**: the gate exists only in the
    uncommitted working tree, so the sentence licensing trust in the page describes a build
    that did not produce this page.

#### Contradictions within the page

193 vs 2917-2921; 1571 vs 1560; 1750 vs 1739; 2154-2156 vs 2966-2968; 2183 vs 2970-2972
(the page ships the error it documents as an error); 189 vs 2899-2901; 197 vs 2277-2280.

#### Gates that cannot fail

- **(a) The whole `affects`/`caveat`/`corrections` subsystem is disconnected from the page.**
  Its gates run and can fail, but the JSON projection at `generate-console-data.mjs:2469-2480`
  emits only ten keys and drops `caveats` and `corrections`. Confirmed empirically:
  `"caveats" in method` is **false** for every method in the shipped JSON. Findings 1, 2, 3,
  4 and 12 all passed straight through gates that police discarded values.
- **(b) The renderer reimplements the exact anti-pattern the data layer forbids by name.**
  `generate-console-data.mjs:1116-1120` sets `affects: []` with the comment "attaching it to
  novai_faucet because both sentences contain the word faucet is exactly the mistake
  `affects` exists to prevent". `generate-console-html.mjs:422` then keyword-scans exception
  prose for method names.
- **(c)** `assertInheritedMeaningIsTrue` is absent from the committed generator, so it has
  never gated anything.
- **(d)** The four-way drift gate is real and reachable and calls `process.exit(1)`. Its only
  defect is scope: names, and nothing else.
- **(e) Not a defect, recorded because it was hunted for:** the shared-`/g/`-regex bug is
  fixed and fixed correctly. `subPattern()` at `:104` is a factory, `hasForbidden` at `:105`
  builds a non-global regex for `test()`, and `errorCodePattern` at `:797` follows the same
  discipline. No shared `/g/` regex reachable from both `test()` and `matchAll()` exists.

#### Verified with evidence, read today

All 29 `rpc.rs:NNNN` citations correct, checked mechanically with a poisoned-control
instrument. 29 methods across all four sources, `disagreements: []`. All six limits, all
eleven minimum fees and discriminants (contiguous 1..11), three bps constants, 23 signal
types (`ai_entities/src/signals.rs:13`), 16 memory object types (`memory.rs:335`), seven
capability bits, reputation and stake constants, `TX_V1_OVERHEAD = 149`
(`codec/src/lib.rs:231`), `PRUNE_RETAIN_BLOCKS = 50_000`, both quorum citations, and all
nine SDK coverage numbers counted from SDK source rather than any README, including the
genuinely absent TypeScript `entityUpgrade`. Section 09's pruning measurements reproduce
live. Both faucet drift exceptions are still real today.

#### Unverifiable, honestly marked

Validator count 4 (not exposed over RPC; both genesis files name 5, consistent with the
page's own note). The `-32002` pruned-transaction band, which the page itself says cannot
be shown externally. The PyPI publication date. All write-path claims in section 06, since
it was barred from `submitTransaction` and `novai_faucet`.

---

