# Website handoff

Written 2026-08-27. Covers BOTH website workstreams, because they are paused at
different points and a fresh session that knows about only one will break the
other.

Read this before touching anything under `website/`.

**Status: 17 commits ahead of origin/main, none pushed. Pushing auto-deploys to
Cloudflare. Do not push.** One of the seventeen, `1842fb7`, is node work rather
than website work.

Two workstreams share this directory:

1. **Site redesign**, paused mid-ladder. Gates 2.5, 2, 3 and 4 landed. Gates 5
   to 9 pending.
2. **Developer console** at `novai.network/console`, a separate page. Gates C0
   and C1 landed. Gates C2 to C6 pending.

They share the token system, the console component vocabulary, the chain hook,
the design-rules test and the build. A change to any of those touches both.

---

## 1. Hard rules, verbatim

These are the operator's words and they override any default behaviour.

1. **DO NOT PUSH. Ever.** Push auto-deploys to Cloudflare.
2. **No AI attribution anywhere.** No `Co-Authored-By`, no AI markers in
   commits, comments or headers.
3. **No commits until a gate is approved, and ask even then.**
4. **Everything local.** The only review artifact is localhost.
5. **Zero chain code.** `website/` only, plus scripts under `website/scripts/`
   that READ the repo and write only inside `website/`.
6. **Never touch the server.** Server needs go to `website/NEEDS-OPERATOR.md`.
7. **No PII.** No absolute home-directory paths, no real name, no server IP, no
   cross-project domains. (The original rule spells out the home-path prefix and
   names two specific projects. Both are paraphrased here, because quoting them
   would make this file a search hit for exactly the strings the rule exists to
   keep out of the repository, and the pre-commit hook rejects them on sight.
   This is the only place in this document where the rules are not word-for-word.)
8. **No em dashes, no en dashes.** The gate enforces it.
9. **First person singular** in prose. Never "we".
10. **Never assume. Stop and ask.**
11. **`/scrub --staged` before every commit.** `git add` and `git commit` never
    bundled.

Console-specific addition:

> **This page is PUBLIC. Treat every string on it as published.** Never publish:
> any IP address, any node hostname, anything identifying which node serves the
> RPC, internal network topology, Prometheus or metrics endpoints, filesystem
> paths, internal port numbers, the operator's real name, or any cross-project
> domain. If unsure whether a value is safe, ask. Do not reason your way to yes.

Commit identity must be `NOVAInetwork <251796703+0x-devc@users.noreply.github.com>`.

---

## 2. Commit ledger

All 13 are local only. Oldest first.

### Site redesign

| Hash | What |
|---|---|
| `2706aeb` | Gate 2.5 floor: dash gate, dead pages and router removal, repo stats generator |
| `4e32c47` | Gate 2.5 floor: dropped the wrong blocks-committed stat, site numbers now derived from the repo |
| `6bf88fd` | Gate 2.5 floor: ship `novai-symbol.png` so og:image and twitter:image resolve |
| `7965e01` | Gate 2: design system v2 tokens, motion primitives, dev specimen |
| `2a95fd4` | Gate 3: live chain data on the specimen through a dev-only proxy |
| `7675c14` | Gate 4B: verify panel, chain queries and hash-linkage proof in the browser |

Older commits on `website/` (`6fcffad`, `0d62269`, `4d1d919`, `acd34fe`,
`48b51e1`, `c179886`, `30d30f3`) predate the redesign and are already pushed.
`c179886` is the commit that introduced the "30M+ Blocks Committed" claim later
deleted by `4e32c47`; it is the origin of the config-facts rule in section 4.

### Developer console

| Hash | What |
|---|---|
| `c87057e` | C0: console component vocabulary (Panel, Headline, KVGrid, DataTable) and `useInView` |
| `afa6bb1` | C0: time-span rate window and caller-selected poll cadence |
| `4df576b` | C0: panels rebuilt on the vocabulary, and never seed a query height from the build snapshot |
| `8d41f24` | C0: snapshot freshness gate and one-command `predeploy` |
| `581564b` | C0: operator actions rewritten (CORS resolved, doc drift, hosting, funding) |
| `e2f4647` | C0: enable `strictNullChecks` and gate the suite on typecheck |
| `c102a5f` | C1: multi-page build and the console layout skeleton |
| `dcc7488` | This handoff document |
| (C2, this session) | C2: the console generation pipeline, the four-way drift gate and the four printed drift exceptions |
| (C2, this session) | The dash gate fix, plus the audit of every other shared-regex site |

---

## 3. Where each workstream stands

### Site redesign: gates 5 to 9 pending

Gate numbering follows the supersession the operator sent on 2026-08-24, which
replaced the original prompt from Gate 3 onward. Under it the assistant's role
is Chief Design Engineer with design-quality final say, and the instruction is
to argue rather than simply comply.

- **Gate 5, agent surface.** `llms.txt` and `agents.json`. **This is the item
  most likely to be quietly dropped, so it is called out here deliberately.**
  Neither file exists yet. Neither does `sitemap.xml`. The console needs a
  sitemap and an `agents.json` at its own Gate C5, and the operator ruled that
  the console workstream CREATES both, structured so the main site's entries
  slot in cleanly rather than requiring a rewrite. So by the time Gate 5 runs,
  the files may already exist and Gate 5 extends them. `llms.txt` is written in
  neutral third person, never "we".
- **Gate 6, sections.** Section order is home, vision, pillars, network,
  testnet, roadmap, contribute, documents, socials. Pillars is a section but
  drops out of the nav. Roadmap splits milestones (public and deep) from
  performance numbers, and performance numbers are never targets: only measured
  and labelled.
- **Gate 7, prerender, performance, accessibility, mobile sweep, motion.** The
  motion architecture amendment lives here, see section 6. Also carries the
  unresolved 360px stat-weight question in section 5.
- **Gate 8.** Contents NOT recorded in any source I could reach. **Confirm with
  the operator before planning it.** The pre-supersession ladder ended at
  "8 adversarial verification", but the supersession inserted a gate, so
  adversarial verification is now Gate 9 (below) and whatever Gate 8 holds is
  unknown. Do not assume it is the old Gate 8. The external link check belongs
  to Gate 9, per the operator's own enumeration.
- **Gate 9, adversarial verification.** Verbatim from the operator:

  > Enumerate every realistic failure and prove it does not break:
  > - RPC unreachable, timeout, malformed JSON, JSON-RPC error, result: null,
  >   429 with a non-JSON body, height going backwards after a reset, chain wedged
  >   at one height
  > - VITE_RPC_URL unset entirely
  > - repo-stats.generated.json missing or malformed
  > - agents.json malformed
  > - Hero video missing, corrupt, or slow to load
  > - prefers-reduced-motion: reduce
  > - JavaScript disabled: what the prerendered HTML actually contains
  > - 360px, 768px, 1440px, 2560px
  > - 4x CPU throttle, slow 3G
  > - Every external link still returns 200, including the explorer
  >
  > State plainly what could NOT be tested. Real Lighthouse on a physical phone,
  > real social-crawler rendering, the Cloudflare production environment, and the
  > video variant with a real file are all out of reach. Do not imply coverage you
  > do not have.

  Note that several of these are already covered by existing tests and can be
  cited rather than re-derived: the chain-hook suite already pins RPC
  unreachable, timeout, malformed JSON, JSON-RPC error, `result: null`, 429 with
  a non-JSON body, height regression and the wedged-chain stale flip. Chrome
  cannot reach a 360px viewport by window sizing on macOS; use CDP emulation,
  see section 9.

Also outstanding from Gate 4: candidate A was rejected as built, but a static
chain-axis motif salvage was APPROVED and has not been built.

### Developer console: gates C2 to C6 pending

The Phase 1 plan is approved. Page structure is ten sections in this order:
01 connect, 02 first call, 03 rpc reference, 04 errors and limits,
05 transactions, 06 ai entities, 07 sdks, 08 network parameters, 09 known gaps,
10 verify it yourself.

The ordering test the page must keep passing: a developer must reach a working
transaction without scrolling through the AI section, and must reach the AI
section without reading the transaction section.

- **Gate C2, the generation pipeline. LANDED 2026-08-28.**
  `website/scripts/generate-console-data.mjs` emits
  `website/src/data/console-data.generated.json` and
  `website/public/openrpc.json` from one parse, so the two cannot disagree.
  Follows the `generate-repo-stats.mjs` pattern: pure Node, no network, no git,
  no shell-outs, `--root` / `--out` / `--openrpc` / `--check`, provenance
  travelling with the data, `generatedAt` preserved when values are unchanged so
  determinism is a byte compare, atomic tmp-plus-rename, `FAIL:` prefix and exit
  1, `--check` in `prebuild`. Measured coverage, printed on every run: 29
  methods, 29 curls, 25 own params tables resolving to 29, 18 own result fences
  resolving to 29, 21 own error sets resolving to 29, 19 sample responses. The
  gap closes through 7 alias resolutions and 17 methods inheriting a record
  shape from their category preamble. Those two numbers were originally believed
  to be 4 and 11; building to the believed figures would have shipped six
  methods showing a record type name with no definition behind it. Measure the
  document, never the memory of it.
- **Gate C3, the reference content. LANDED 2026-08-29.** All ten sections are
  filled and the layout filler block is deleted. Two commits: the source-derived
  datasets and their gates, then the rendered sections.
  - `website/scripts/generate-console-html.mjs` writes the section markup INTO
    `console.html` between sentinel markers. It is written to the file rather
    than injected by a Vite hook because `tailwind.config.ts` scans
    `./console.html` from disk: markup injected at `transformIndexHtml` time is
    invisible to that scan and every class in it would be purged, so the page
    would look right in dev and render unstyled in production. Verified
    empirically, not assumed: `list-disc` and `pl-5` appear in zero source
    files, three times in the generated markup, and once each in the built CSS.
  - The marker mechanism carries four gates of its own (unforgeable sigil,
    paired markers, region count equals renderer count, and `--check`
    re-renders and compares), each proven by doctoring.
  - Fourteen new datasets are read from `crates/` and `sdk/` directly rather
    than from the reference document, with cross-checks that fail the build.
    There are now five `KNOWN_DRIFT` exceptions, not four.
  - The AI recipe claim strings were approved individually before the section
    was built, as the pre-C3 review required.
  - Section 03 renders all 29 method entries EXPANDED. Collapsing was measured
    and rejected: the whole page is 28 KiB gzipped, so collapse buys nothing,
    and content inside a closed `<details>` is unreachable by find-in-page in
    Firefox and Safari. `<details>` is used only for the two long lookup
    enumerations in section 06.
- **Gate C4, correctness and structure. LANDED 2026-08-30.** The gate was
  renumbered by the operator at the start of that session: what this document
  used to call C4 (live panels) is now C5. C4 became the correctness pass plus
  the page split, because the C3 page was publishing wrong facts and those had
  to go first.
  - **Fifteen defect classes closed.** All of them were downstream of one thing:
    the data generator computed the right per-method caveat, correction and role
    reinterpretation, gated all three, and `payloadFor` then dropped `caveats`
    and `corrections` from the emitted JSON while the renderer rebuilt the index
    Notes column by regex-scanning exception PROSE for method names. That scan
    failed in both directions on one page. `novai_getBalance` carried
    `novai_getNonce`'s caveat, which is the reverse of the truth for getBalance;
    `novai_submitTransaction`, the only state-mutating method, carried none.
  - **The renderer now reads per-method declarations only.** `CAVEAT_LABELS` and
    the prose regex are deleted and a test asserts they are gone, because every
    other check passes on a renderer that still scans prose.
  - **Corrections are struck at their own site** with the truth on the next line,
    rather than listed 2,426 lines away. Struck rather than deleted: the
    sentence is still in `docs/RPC_REFERENCE.md`, which every method row links
    to, so silently dropping it would leave a reader unable to tell curation
    from staleness.
  - **Nine drift exceptions, up from five.** The four new ones are the vk-list
    foreign field, `getNonce`'s unreachable `-32002`, `getBlockByHeight`'s null
    path documented as unreachable, and the `-32600` trigger. `NEEDS-OPERATOR.md`
    items 16 to 19.
  - **Six record shapes now render.** Fifteen of 29 methods published a result
    type the page never defined, while printing "Record shape declared once for
    this category" seventeen times. `basis_points` appeared zero times on the
    page. Each shape is declared once, every reference links to it, and a gate
    refuses any type reference the page does not define.
  - **Eight pages.** All 29 methods stay together on `console/rpc.html`. Plus two
    generated find surfaces, `console/all.html` and `console/names.html`, which
    are how the split keeps one-keystroke findability at zero JavaScript.
  - **Build-time syntax highlighting**, `scripts/tokenise.mjs`, hand written. Not
    a dependency question: `JSON.parse` fails on 29 of the 51 JSON fences because
    they are a schema notation with type placeholders, bare record references and
    comments. A real JSON grammar is less accurate here, and it cannot express a
    record reference as a link, which is the construct that fixes the shape gap.
  - **One chart**, the fee ladder, and three candidate datasets were read and
    rejected. **Copy buttons**, injected by the only script on the page, so with
    JavaScript off the page is what it was.

- **Gate C5, live panels.** Network status and verify panel as React islands,
  mounted (not hydrated) into containers holding identical static markup, so
  hydration mismatch is not a possible failure mode. Needs a new RPC endpoint
  resolver module: none exists, `PUBLIC_RPC_URL` currently has exactly one call
  site and it is the curl string builder. Was C4 before the 2026-08-30
  renumbering.
- **Gate C6.** Prerender, performance, accessibility, 360px, `noindex`,
  `sitemap.xml`, `agents.json`, tests. The sitemap now has ten console URLs to
  carry, not one.
- **Gate C7.** Adversarial verification, including the doctored-doc test that
  must fail the build.

#### The KNOWN_DRIFT exception mechanism, and why it must fail on a FIX

This is the heart of Gate C2 and it is easy to get subtly wrong.

The console generates its reference material from `docs/RPC_REFERENCE.md` and
cross-checks it against the implementation. The check is a **name set** equality
across four independent sources, not a count:

1. dispatch arms in `crates/node/src/rpc.rs`
2. `### ` headings in `docs/RPC_REFERENCE.md`
3. the method table in `README.md`
4. the methods the Python SDK client actually calls

All four agreed at 29 when last measured. The fourth is the strongest signal
because it is executable code rather than prose.

Three real doc defects exist today (section 5). The operator's ruling was that
they ship as explicit, individually justified exceptions in a `KNOWN_DRIFT`
list, printed on **every** run, each carrying a `NEEDS-OPERATOR.md` reference.

**The gate must fail in BOTH directions:**

- it fails when a NEW drift appears, which is the obvious half, and
- **it fails when a listed exception STOPS applying**, naming the exception to
  delete.

The second half is the entire point. Without it the list becomes furniture: the
doc gets fixed, the exception silently keeps suppressing a check that now has
nothing to suppress, and the list only ever grows. With it the list can only
shrink, and fixing a doc actively forces its exception to be removed. Anyone
tempted to "simplify" this by only failing on new drift is deleting the
mechanism.

Hardening the dispatch scan against false passes (all verified as real risks):
locate the match block by brace-slicing rather than by indentation anchor so a
rustfmt change cannot disarm it; strip line comments before scanning so a
commented-out arm cannot count; assert every arm is a bare literal with no
or-pattern and no guard so an alias cannot hide; assert `rpc.rs` is the only
file in `crates/` containing a dispatch-shaped line; assert total arms minus the
catch-all equals the `novai_`-prefixed count so a method added under another
prefix cannot pass; assert the method name embedded in each curl equals its own
heading. Note `rpc.rs` contains three doc-comment references to
`novai_getStatus`, a method that does NOT exist, so any regex loose enough to
scan the whole file injects a phantom method.

Also required: the generator must normalise U+2212 to an ASCII hyphen.
`docs/RPC_REFERENCE.md` contains exactly one, and `check-dashes.mjs` forbids it,
so copying doc text verbatim into `website/` fails the build.

Parser reality, measured: all 29 methods have a curl, but only 25 have a params
table, 18 have a result fence, 21 have error codes and 19 have a sample
response. Alias resolution and per-category shape inheritance are mandatory. A
naive parser produces a page that is silently wrong for about a third of the
surface while looking complete.

---

## 4. Settled decisions. Do not reopen these.

- **10 second poll cadence, and it binds BOTH workstreams.** Public pages poll
  the RPC at 10s in-view, paused in hidden tabs. Not 2s. The 2s cadence was
  right for a single-viewer dev specimen; on a public page each concurrent
  viewer costs 0.5 req/s against a 100 req/s per-IP cap SHARED with the
  explorer, so roughly 200 concurrent readers saturate it for everyone. The
  site's `#network` section inherits this. Do not leave 2s in the site plan.
- **Never hardcode a cadence figure anywhere.** See section 7.
- **Derive, never type.** Every public number is either derived from live data
  at read time or is a configuration fact that cannot silently go false.
- **No cumulative counters, and no operational adjectives as static text.**
  "active", "running", "passing" are banned unless live-data-driven. Say
  "4-validator", "11 transaction types", "2,100+ tests", never "tests passing".
- **Never ship a rate that implies transaction load when blocks are empty.**
  Label it block cadence or drop it.
- **`strictNullChecks` is ON and gated in `npm test`.** `npm test` runs the dash
  gate, then `typecheck`, then the suite. Scoped deliberately: full `strict`
  would pull in `noImplicitAny` and `strictFunctionTypes` and become a refactor
  across files due to be rewritten anyway. Enabling `strictNullChecks` alone
  cleared all nine pre-existing errors and introduced none. Those nine were all
  the same defect: without it, TypeScript does not narrow a discriminated union,
  which is exactly what the RPC outcome types depend on.
- **`predeploy` refreshes the snapshot rather than only checking it.** One
  command answers whether a deploy is clear:
  `npm run snapshot && npm run snapshot:check && npm test && npm run build`.
  It refreshes because the retention window is a few hours of wall clock, so a
  check-only command would fail on essentially every invocation and would stop
  being read. A gate that cannot go green gets disabled. It WRITES
  `src/data/chain-snapshot.json`, which is committed as part of deploying, and
  it is deliberately NOT in `prebuild` because builds are hermetic and network
  free.
- **The build snapshot is a labelled historical display only.** Nothing may
  treat a snapshot value as retrievable. The verify panel's height input is
  never seeded from it: a node retains only 50,000 blocks, so a seeded height
  drifts past the pruning horizon and answers null on the visitor's first click.
- **Tables and KV grids size to their content, never `w-full`.** "Use the width"
  was a complaint about a narrow centred column with wasted margins, not an
  instruction to stretch every element to the viewport. The page uses the width;
  data stays dense. Both the static markup and the React components carry this.
- **Bundle claims are reported as "byte-identical app JS, plus these deltas with
  their causes", never as "unchanged".** The operator asked for this style to be
  kept for every future bundle claim.
- **The console ships zero JavaScript today** and its reference sections are
  static HTML, NEVER React. One rendering path per section, so no hydration
  boundary can mismatch and no second copy can drift. React is for the two
  islands only.
- **`index.html` must stay named in `rollupOptions.input`.** The moment `input`
  is set it stops being the implicit default; omitting it silently deletes the
  marketing site from the build. `specimen.html` stays absent from that map,
  which is what keeps it dev-only.
- **Tailwind scans `console.html`.** Its content globs are `.ts`/`.tsx` only;
  without the HTML entry every class on that page is purged. This costs 311
  bytes gzip on the shared CSS and the operator explicitly ACCEPTED that rather
  than run a second Tailwind pass, which would be a new class of drift bug for a
  third of a kilobyte. Revisit only at site Gate 8, with measurements.
- **CORS is open.** `rpc.novai.network` returns `Access-Control-Allow-Origin: *`
  (verified by preflight, by a real POST, and by an invalid origin receiving the
  same wildcard, which distinguishes a true wildcard from an origin echo). This
  is a live observation, NOT a configuration fact, so the terminal-mode fallback
  stays built and tested.
- **The explorer is a separate deployment. Do not touch it.** Link markup for it
  goes in `NEEDS-OPERATOR.md`.
- **Console link placement** goes in the Documents section of the main site as a
  link, not in the nav. Proper nav placement is a site Gate 7 item.
- **Meta `noindex` only, no `robots.txt` `Disallow`.** A `Disallow` prevents
  crawling, which means the crawler never reads the `noindex`, and it blocks
  `openrpc.json` from well-behaved agents, which cuts against the whole
  agent-surface thesis. Crawlable but not indexed is the intended state.
- **Excluded from the console permanently:** repo statistics (line counts, test
  counts), block browsing, transaction search, address lookup (all the
  explorer's job), and a visible faucet.

---

## 5. Open problems

- **Empty blocks, unresolved.** `tx_count` is 0 and the state root does not
  change between blocks. Any rate shown must be labelled block cadence, never
  transaction throughput. The honesty question about what to display was raised
  and has not been closed.
- **Chain id unknown.** Two unrelated things carry the name: a protocol constant
  equal to 1 used only in channel-state signing (must NOT be published, it would
  be read as the network identifier), and a human-readable genesis string that
  takes three different values across the devnet, testnet and mainnet configs.
  In `--dev-keys` mode the node reads no genesis file at all, so none of the
  three is necessarily what is running. **Omitted from the console until the
  operator confirms the live value.** `NEEDS-OPERATOR.md` item 3.
- **Genesis hash omitted because block 0 is pruned.** It is computed at runtime,
  not a repo constant, so it cannot be generated. Deriving it from the chain
  also fails: block 0 and block 1 both answer `null`, because
  `PRUNE_RETAIN_BLOCKS` is 50,000 and genesis fell out of the window long ago.
  The console instead states, under known gaps, that block 0 is not retrievable,
  which is the more useful fact. `NEEDS-OPERATOR.md` item 4.
- **Faucet key status unknown.** The public HTTP route `GET /faucet/<address>`
  is gated solely on `--faucet-key`, NOT on dev mode. Whether that flag is set
  on the live testnet decides whether the console documents a funding path or
  states plainly that none exists. Build no faucet UI either way.
  `NEEDS-OPERATOR.md` item 2.
- **Four drift exceptions, as shrinking debt.** Each has a `NEEDS-OPERATOR.md`
  entry, each is printed on every generator run, and each must be deleted when
  the doc is fixed. The gate fails when a listed exception stops applying,
  naming the entry to delete, so the list can only shrink:
  1. `-32014 NonceTooHigh` is emitted at `crates/node/src/rpc.rs:2060` and
     appears zero times in the doc. This is client-breaking, not cosmetic: the
     doc's guidance for the codes it does list would make a client resync when
     the correct handling is to retry unchanged.
  2. `GET /faucet` gating is documented backwards. The doc says dev-mode only;
     the handler takes no dev-mode parameter and gates on `--faucet-key`, so it
     runs in production and does NOT run on a plain dev-keys devnet.
  3. `novai_faucet`'s disabled path returns `-32000`; the doc's table says
     `-32602`.
  4. `novai_faucet` is documented as available only under `--dev-keys`, but
     `handle_faucet` prefers a loaded `--faucet-key` and only falls back to the
     dev key, so the method runs on a production node. This is defect 2 again on
     the JSON-RPC surface rather than the HTTP route. `NEEDS-OPERATOR.md` item
     13.
  Two further doc omissions are recorded but are not gate exceptions: every
  example assumes a loopback endpoint (the 29 curls themselves use `$URL` and
  are portable, so this is a one-line prose fix and probably the highest-value
  edit in the document), and the Observed gaps table never mentions that history
  is pruned.
- **300-weight stat at 360px is still unjudged.** Carried from site Gate 2. The
  operator could not reach the dev server from a phone. Open item for site
  Gate 7 mobile QA. Do not guess it.
- **Candidate C, the treemap, is CLOSED as rejected** (2026-08-28). It was held
  rather than rejected for a long time, so the reasoning is recorded here to
  stop it being revived. A treemap sizes crates by line count, which answers
  "which crate is biggest". Nobody learning a system asks that, and on the
  marketing site it is the same vanity-stat class that `4e32c47` deleted. The
  same 16 crates rendered as the layered dependency DAG answer the question a
  learner actually has, so the DAG replaces it and moves to the console, where a
  crate map is a LEARN artifact rather than a marketing number.
- **Console table scroll affordance. CLOSED at C3.** Solved in pure CSS with the
  background-attachment technique: two `local` layers scroll with the content
  and cover two `scroll` shadow layers, so an edge shadow shows only while
  there is more to reach in that direction, and it disappears by itself when
  the table fits. No JavaScript, so it works on a page that ships none.
- **TypeScript SDK is silently truncated:** 7 signal types against the chain's
  23, 5 memory object types against 16, 10 transaction builders against 11 with
  no `entityUpgrade`. Nothing in the type system catches this. It is published
  as an honest coverage matrix and TS is marked in development and not a
  supported signing path. The internal signing-audit status is deliberately NOT
  published: state the coverage gap, which is the fact, and omit the process
  status, which is not the public's business.
- **SDK quickstarts have not been run.** The operator's instruction is to run
  them and to publish no quickstart that has not been verified, because a
  quickstart that fails is the most trust-destroying content on the page.
  Authorised footprint: build the Rust workspace and a local devnet, artifacts
  confined to gitignored paths, devnet processes cleaned up, nothing touching
  the production fleet. If the workspace build fails, report it and STOP; do not
  work around it and publish anyway. A verified read-only quickstart against the
  public endpoint is acceptable, with the write path clearly labelled as a
  local-devnet walkthrough.
- **Website tests run in no CI.** The GitHub workflow is Rust-only. The dash
  gate, typecheck, contrast audit and suite are local-only.

---

## 6. Motion debt

The site currently has an approved architecture of **exactly three bespoke-motion
sections**: the hero, the network live-data motion, and the roadmap progression
rail. Everything else is quiet on a single fade, deliberately: vision, pillars,
testnet, contribute, documents, socials.

The supersession allows this to **rise from three to five bespoke sections at
Gate 7**, and the instruction is to argue the case rather than simply comply.

The debt: **candidate A was the only page-level pacing candidate and it was
rejected.** Its static chain-axis motif salvage was approved but not built. So
the five-treatment motion proposal at Gate 7 must now carry the page-level
pacing that candidate A would have provided. That is a requirement on the
proposal, not an optional extra.

**The approved, unbuilt salvage from candidate A (site Gate 7).** When candidate
A was rejected as built, a static chain-axis motif was approved in its place and
has never been built. The shape: the hero's hairline datum recurs as the
roadmap's rail and as the network panel's rule, one continuous line running
through the page at zero runtime cost.

This matters more than its size suggests. It is the ONLY thing partially
covering the page-level pacing gap that candidate A's rejection left, and it
does so with no animation budget at all. It should be built before the
five-treatment proposal is argued, because it changes what that proposal still
needs to carry.

Structural variety comes from framer-motion ONLY. No GSAP, no Lenis, no Lottie,
for bundle, reduced-motion and anchor-nav reasons. Component libraries are
reference-only, never paste, because of token conflicts and licence risk.

The console has almost no motion by design: state transition crossfades, the
live dot pulse, and the odometer on height change. That is the entire list.

---

## 7. The chain moves faster than any figure you remember

Measured block cadence went from **1.13 blocks/sec** at the start of this work to
**4.77 blocks/sec** within days. That is not a rounding difference, it is a 4x
change, and it moves the 50,000 block retention window from roughly half a day
down to under three hours.

Consequences a fresh session must internalise:

- **Any cadence figure in any document, including this one, is probably already
  wrong.** Measure it; do not quote it.
- The retention window is a configuration fact in BLOCKS
  (`PRUNE_RETAIN_BLOCKS = 50_000` in `crates/consensus/src/lib.rs`). The
  wall-clock equivalent is derived and unstable. Publish the blocks figure.
- A committed chain snapshot is usable for a matter of hours. This is why
  `predeploy` refreshes rather than checks.
- This is the concrete reason behind the derive-never-type rule. It is not
  theoretical fastidiousness.

---

## 8. The four hero fallback checks

Verbatim from the operator.

> The coded hero is rejected and the operator switches to externally generated
> video if ANY of these are true:
>
> 1. It is a full-viewport dark gradient with floating particles and a centred
>    gradient-text headline, which is the generic template this site is trying to
>    escape.
> 2. It is visually indistinguishable from the current ParticleField plus GlowOrb
>    combination.
> 3. It carries no information. The hero visual should encode something real about
>    the chain, not decorate.
> 4. It cannot hold 60fps on a 4x-throttled CPU.
>
> Gate 3 must state these four checks explicitly in its report and self-assess
> against each one honestly. A failed check is not a failure of the build, it is
> the trigger for plan B: externally generated video that the operator supplies
> and drops into website/public/hero/ (gitignored, never committed).

**Check iii is the load-bearing one, and it was the one that could not be
recovered from any surviving note.** It is also the most consequential of the
four, because it is a design constraint rather than a performance budget and it
cannot be satisfied by tuning.

Two decisions already trace directly to it:

- It is why the persistent chain-axis candidate was rejected. A line that is
  always there is decoration; it states nothing that changes.
- It is why Quorum Nova drives its commit cadence off the REAL observed block
  rate rather than a fixed timer. The animation tempo being actual chain data is
  what makes the visual informational rather than decorative. Anyone
  "simplifying" that to a constant interval fails check iii and does not notice,
  because it looks identical in a static screenshot.

A hero that passes i, ii and iv while failing iii is still rejected.

## 9. Practical notes for a fresh session

- Baseline gate state after C4: dash gate clean, css-coverage clean, typecheck
  clean, contrast audit zero gating failures, 149 tests of which 146 pass.
  **All three failures are one root cause and none of them is a console defect:
  a generated file describing a `crates/` tree that is being edited right now.**
  1. `repo-stats` staleness. Refreshed during the C4 session and immediately
     stale again: it was measured against a working tree carrying uncommitted
     node work, so 130,785 lines and 2,175 tests describe code that is not in
     the repository, and the tree moved again within minutes.
  2. `console-data --check`. A fresh run puts `MAX_INDEX_ENTRIES` at
     `crates/node/src/main.rs:362`; the committed data says 339. The difference
     is uncommitted work above that line. **The committed 339 was kept
     deliberately.** Source links point at `blob/main`, so they must match
     committed code; regenerating would publish a link to a line that exists in
     no commit.
  3. `prints every exception on a successful run` fails only because it shells
     out to `--check` and inherits failure 2.
  None of these can be made green from inside `website/`. They go green when the
  node work is committed and the two generated files are refreshed against it,
  which is one command each and an operator decision because both publish
  numbers.
- **That warning came true on 2026-08-30 and is worth reading twice.** The
  stats file was refreshed while node work was uncommitted, so it now publishes
  `linesOfRust` and `tests` measured against a tree the repository does not
  contain. Refreshing a derived file is only safe when the thing it derives from
  is committed. Both generated files should be regenerated in the same commit
  that lands the node work, not before it and not after.
- `npm test` = dash gate, css-coverage config check, typecheck, suite.
  `npm run build` runs `prebuild` first: dash gate, repo-stats check,
  console-data check, console-html check, css-coverage config check.
  `npm run predeploy` is the full chain and finishes with
  `check-css-coverage --dist`, which is the only check that reads the BUILT
  output rather than the config's intent. **`prebuild` is currently red for the
  two reasons in the list above, so `npm run build` will not run until the node
  work is committed; `npx vite build` builds fine.**
- **`scripts/check-css-coverage.mjs` exists because the console is ten HTML
  files now.** A page missing from Tailwind's content globs has every utility
  class on it purged, and the failure is production-only: dev does not purge, so
  it looks perfect locally and ships unstyled. The config half checks that every
  HTML entry carrying its own classes is matched by a glob; the output half
  checks that a class in each BUILT page resolves in a stylesheet that page
  links. Only the second would have caught the original bug.
- The design-rules test is the erosion guard. It walks `src/` for `.ts`, `.tsx`
  and `.css` only, so it does NOT see `console.html`; the console rule reads
  that file explicitly. Allowlists are keyed on exact relative paths, so moving
  an allowlisted file (notably `StatusDot.tsx`) breaks the test unless the
  allowlist moves with it. `LEGACY_EXEMPT` must not be added to without a gate
  decision.
- Chrome on macOS clamps its window to about 500px, so `--window-size=360` does
  NOT give a 360px viewport and will look like catastrophic overflow that is not
  real. Use CDP `Emulation.setDeviceMetricsOverride`. Node 24 has a native
  `WebSocket`, so a CDP driver needs no dependencies.
- The chain snapshot is in the scrub ignore list by operator ruling, so its hex
  values do not raise findings.
- `docs/` is no-touch. The local-only agent instruction file at the repo root
  is gitignored and must never be committed; the pre-commit hook rejects its
  filename, so do not name it in a tracked file either.
- Gate reports state what was done, what was NOT done, and what is uncertain,
  then STOP and wait.

---

## 10. Learnings

Things that were expensive to find and are not obvious from the code.

### The dash gate never worked

One module-level `/g/` regex shared between `test()` and `matchAll()` meant
`lastIndex` carried over, so every line was scanned from past the violation.
Concealed because `check-dashes.mjs` defines the forbidden characters itself and
is the first file the walk reaches. Every "dash gate: clean" recorded before
2026-08-28 was unreliable, including in commits already made. Found not by
review but by building a second generator that had to touch the same characters.
The first attempt to PROVE the fix also reported clean and was also wrong: the
probe file had a leading dot and the walk skips dotfiles.

Generalisation: a gate that can only be exercised by its own definition site
cannot be trusted, and a proof of a gate needs its own proof that the probe was
actually seen.

The fix builds the regex fresh at each use, and the definition-site exemption is
two conditions, a path allowlist AND a per-line marker, so it cannot be used to
silence an ordinary violation. The allowlist fails when an entry stops being
needed, on the same only-shrink principle as `KNOWN_DRIFT`.

The rest of the tree was audited for the same class on 2026-08-28. `testRe` and
`unsafeRe` in `generate-repo-stats.mjs` are shared `/g/` regexes reused across
every file in the walk, and they are safe only because `String.prototype.match`
resets `lastIndex` to zero both before and after (verified empirically, and
noted in a comment at the site so nobody converts those calls to `matchAll`).
`errorCodePattern` in `generate-console-data.mjs` was safe by accident and is
now a factory. Nothing under `src/` holds a reusable regex object at all: every
hit is an import path, an SVG `<g>` element, or an inline literal built fresh at
each call. No other instance of the bug exists in the tree.
