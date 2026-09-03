# Website handoff

Written 2026-08-27. Status block updated 2026-09-03 at the close of Gate C4b.
Covers BOTH website workstreams, because they are paused at different points and
a fresh session that knows about only one will break the other.

Read this before touching anything under `website/`.

**Status: 5 commits ahead of origin/main, none pushed. Pushing auto-deploys to
Cloudflare. Do not push.** `origin/main` is `98d2e52`. The five unpushed are
`fdba5ad` (C4 record and C4b brief), `169fc0e`, `9e5054b`, `17ff864` (the three
C4b work commits) and the commit carrying this update.

The earlier "17 commits ahead" figure in this block was stale: the C4 commits
`c4a9f3b` and `2454da1` are ancestors of `98d2e52` and are on origin, as is the
node work `1842fb7`. Checked with `git merge-base --is-ancestor` rather than
carried forward. Two separate sessions have now inherited that wrong number and
one acted on it, so verify this line rather than trusting it.

Two workstreams share this directory:

1. **Site redesign**, paused mid-ladder. Gates 2.5, 2, 3 and 4 landed. Gates 5
   to 9 pending.
2. **Developer console** at `novai.network/console`, now ten pages. Gates C0
   through C4b landed. Gates C5 to C7 pending.

**Production does not serve the console.** Every path on `novai.network`,
including ones that cannot exist, returns the same 1,295-byte SPA catch-all body,
and the deployed `<title>` still carries an em dash the dash gate would now
reject. So nothing in this workstream is public, and the deployed site is older
than the redesign. See `NEEDS-OPERATOR.md` items 6 and 23.

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
| `c4a9f3b` | C3 render plus C4 correctness: 15 defect classes, 4 gates, 9 drift exceptions |
| `2454da1` | C4 structure: 8 pages, 2 find surfaces, tokeniser, colour, chart, layout, 3 gates |
| `fdba5ad` | C4 record: freeze the console, the Phase 3 verification, the C4b brief |
| `169fc0e` | C4b instruments: gate the snapshots in `npm test`, commit the CDP driver |
| `9e5054b` | C4b data: the row splitter, 2 table gates, the shared error-provenance accessor, category-common scoping, verbatim clauses, 2 new drift exceptions |
| `17ff864` | C4b renderer: the missing lead sentence, 4 dead gates made to fail, the cross-reference gate, find-surface identity, measured citations |

`c4a9f3b` and `2454da1` are on origin. Everything below them in this table is
local.

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
- **Gate C4, correctness and structure. COMPLETE 2026-08-30, `c4a9f3b` and `2454da1`, neither pushed.** The gate was
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

- **Gate C4b, the thirteen. PENDING. See `C4B-BRIEF.md` and `PHASE3-REPORTS.md`.**
  C4's own adversarial pass attacked 341 claims and falsified 13. It re-found
  none of the fifteen closed defects as still broken, and confirmed several of
  the fixes against the running node. What it found instead: **three defects C4
  introduced, three holes in gates C4 wrote, and eight cross-references the page
  split broke.** The four priority items, in order:
  1. The canonical `entity_id` derivation is published truncated at a dangling
     backslash, because the table parser splits on the document's escaped pipes.
     `code_hash || creator` occurs zero times on the whole console, and
     `entity_id` is the required parameter of fourteen of the twenty-nine
     methods.
  2. `novai_getSignalsByHeight` publishes a range-check error its handler cannot
     emit, behind two independent gate holes: category-common error tables carry
     no `resolvedFrom` key so the inherited-meaning check skips them, and
     `backtickedIdents` cannot parse the clause that carries the defect.
  3. Eight cross-references that read correctly on one page and broke when it
     became eight, including two promises the page makes about itself: a
     conversion the target page states it will never do, and citations a page
     claims to give and does not.
  4. Two C4 fixes applied only half way: a second hardcoded "Five discrepancies"
     inside a `PROSE` string against a derived nine, and four dead anchors on
     `names.html` marked as the current page.
  Nothing is public: the console carries `noindex` and the site does not link to
  it. This is debt, not an incident.

- **Gate C4b, the thirteen. COMPLETE 2026-09-03.** Four commits, none pushed:
  `169fc0e` instruments, `9e5054b` data layer, `17ff864` renderer, and the
  commit carrying this note. All thirteen findings are closed, plus a
  fourteenth found at plan time and six further instances found by sweeping the
  classes rather than the list.
  - **The fourteenth is the one worth remembering.** `renderConnect` rendered
    `PROSE.connect`, a key that has never existed. `rich(undefined)` returns the
    empty string, so the console shipped an empty `<p class="console-lead">` and
    **the opening sentence of the whole developer console was missing from the
    page split until now.** Nobody saw it because nothing was wrong on screen;
    there was simply nothing there.
  - **The data layer.** The table parser split rows on the document's escaped
    pipes, publishing the canonical `entity_id` derivation truncated at a
    dangling backslash. Two independent gates now cover it, one structural (cell
    count against header width) and one semantic (no dangling operator, no
    unbalanced backtick), because three of the thirteen were gate holes and one
    rule per defect is not enough. Measured across all 48 tables: one row failed
    before, none after.
  - **A rule replaced a special case.** `getSignalsByHeight` published a range
    error its handler cannot emit. The document was NOT wrong: it scoped that row
    with "(range queries)" and the console dropped the qualifier when flattening a
    category-common table. A common error row is now inherited only when every
    backticked field identifier in its clause is a parameter of the inheriting
    method. No doc change, no tenth exception.
  - **Error clauses are quoted verbatim.** The reference writes one U+2212 and
    the console normalised it while publishing U+2264 faithfully ten times on the
    same rows. Both files stay ASCII; the reader gets the character the document
    uses.
  - **Two new document defects**, both measured on both sides and carried with
    corrections at the point of the error: `getLatestBlock` claims only global
    errors and emits `-32002` twice, and the `listSlasBySeller` cap sentence is
    false because sellers are not capped in v1. `NEEDS-OPERATOR.md` items 21 and
    22. Eleven exceptions now, up from nine, and the count on the page derives.

#### The learnings from C4b, phrased generally

1. **Run 2's thirteen was measured with an ungated artifact and an uncommitted
   tool.** The frozen snapshots the falsifier attacked were not checked by
   anything in `npm test`, so they could drift from the pages they claimed to
   represent while the suite stayed green; and every viewport, tab-order and
   focus figure in `PHASE3-REPORTS.md` came from a browser driver written ad hoc
   and never committed, so none of it was reproducible. Both are fixed in
   `169fc0e`. The number is softer than it reads, and a measurement is only as
   good as the instrument's provenance.

2. **A gate can exist and not work in four distinct ways, and a test can do it in
   a fifth.** All five have now been found in checks this project wrote
   deliberately, which is the point: these are not sloppiness, they are the
   natural failure modes of verification.
   - **Blind by a key that is never set.** Three checks opened with
     `if (!m.errors?.resolvedFrom) continue;`, and a category-common error table
     carries no such key, so every method inheriting by that route was invisible.
   - **Unable to parse the thing it polices.** `backtickedIdents` required a
     whole backtick span to be one identifier and returned nothing for
     `` `end_height - start_height > 10000` ``, the one clause that carried a
     defect.
   - **Tautological.** `assertLossless` was handed a single token the caller had
     just built, then checked that it rejoins to its own input and carries a
     class. Both true by construction. Three separate instances of this pattern
     have now been found.
   - **Written and then discarded.** `assertProseIsAllUsed` computed the list it
     needed for the reverse check and threw it away with `void declared;`. It
     proved declared-implies-used and not the converse, which is exactly the
     direction that would have caught the missing opening sentence.
   - **A TEST that never reaches the code it claims to exercise.** Of the
     eighteen injection tests written this gate, three would have passed for the
     wrong reason: one injected into a `PROSE` key nothing rendered, so the
     unused-key branch fired first; one injected inside a generated region, so
     the marker check rejected the file before the gate under test ran; and one
     counted every link on a page and so measured the shell chrome rather than
     the index. Each went red, and none went red for the reason it claimed. The
     `landed` flag caught all three. **Assert on the gate's own failure message,
     never merely on a non-zero exit, and check that no earlier gate could have
     rejected the doctored input first.**

#### Recommended for a later gate, assessed and deliberately NOT built in C4b

- **Pin source links to the generating commit SHA rather than `blob/main`.**
  Anything emitting a `blob/main` link should derive from committed state, and
  nothing enforces that today. `generate-console-data.mjs` reads `crates/` from
  disk and publishes line-anchored links, so a dirty tree publishes `#L362` for a
  line that exists in no commit anyone can fetch. That is the trap that kept
  `MAX_INDEX_ENTRIES` frozen at 339 through all of C4. Pinning fixes a live rot
  vector as well, since `blob/main` anchors drift silently every time main moves,
  and it makes the dirty-tree case fail loudly rather than publish a bad link,
  because uncommitted content has no SHA to pin.

  **OBSERVED CASE, 2026-09-03. This is evidence, not a hypothetical, and it is
  the strongest argument for pinning. Do not let it decay into an anecdote in a
  session log: the failure it describes is silent, resolves in a browser, and
  looks correct.** At the close of C4b, `console-data --check` went red without
  any input of mine changing.
  The cause was live chain work in the operator's tree: uncommitted edits to
  `crates/execution/src/lib.rs` had moved `MIN_FEE_TRANSFER` down 23 lines, so a
  fresh run wanted to publish `feeLine: 12348` where the committed data says
  `12325`. Verified rather than assumed:

      git show HEAD:crates/execution/src/lib.rs | sed -n '12325p'
      # pub const MIN_FEE_TRANSFER: u64 = 100;      <- committed, correct
      git show HEAD:crates/execution/src/lib.rs | sed -n '12348p'
      # /// Minimum fee for upgrading an AI entity's code hash ...

  So line 12348 does exist in `HEAD`, and it is a doc comment about a DIFFERENT
  constant. Regenerating would have published a `blob/main` link that resolves,
  loads, and points at the wrong declaration, which is worse than a 404 because
  nothing about it looks broken. **The committed data was left alone.** The red
  check is an artifact of a dirty tree, exactly like `repo-stats`, and the rule
  in section 9 about never regenerating a derived file while its source is
  uncommitted applies to `console-data` more sharply than to `repo-stats`,
  because this one publishes line-anchored links rather than totals.

  The generalisation, which is what makes it worth pinning rather than just
  worth remembering: a `blob/main` line anchor has no failure mode a reader can
  see. A stale one does not 404, it resolves to whatever now occupies that line,
  and a citation pointing confidently at the wrong declaration is the exact
  defect class this console exists to eliminate. A SHA-pinned link either
  resolves to the line that was measured or does not resolve at all.

- **A CI job, which is the real answer to staleness and is worth more than any
  remaining defect.** See section 11.

- **The rail teaches ten sections and the navigation has eight destinations.**
  See section 12.

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
     node work, so 130,785 lines and 2,175 tests described code that was not in
     the repository at the time, and the tree moved again within minutes. Now
     that `98d2e52` has landed, `npm run stats` describes committed state again.
     It publishes site numbers, so it stays an operator decision.
  2. `console-data --check`. A fresh run puts `MAX_INDEX_ENTRIES` at
     `crates/node/src/main.rs:362`; the committed data says 339. **The committed
     339 was kept deliberately while the node work was uncommitted**, because
     source links point at `blob/main` and regenerating would have published a
     link to a line that existed in no commit.
     **That reason expired when `98d2e52` landed.** Line 362 is now the
     committed line, verified with `git show HEAD:crates/node/src/main.rs`, so
     `npm run console:data` is now correct rather than premature. It is one
     command and it turns failures 2 and 3 green. It was left undone only
     because the C4 session was told to stop, not because it is unsafe.
  3. `prints every exception on a successful run` fails only because it shells
     out to `--check` and inherits failure 2.
  A fourth gate is red and is NOT in the suite: `snapshot:check` reports the
  committed chain snapshot at height 4,354,701 against a live tip near
  5,433,000, a gap of about 21 retention windows. It sits in `predeploy`, which
  refreshes before checking, so it self-heals on the next deploy. Worth knowing
  before one, not worth fixing now.
  The node work landed as `98d2e52` at the close of the C4 session, so all three
  are now refreshable with one command each. Both commands rewrite a generated
  file that publishes numbers, so both stay an operator decision rather than a
  side effect of a console commit.
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

---

## 11. CI: turning "cannot rot" into a repo-time property

**DECIDED 2026-09-03, approved in principle by the operator. Not built in C4b.
This is the first item of the next gate.** The shape below is the accepted
design, not a menu: its own workflow separate from `ci.yml`, on push to `main`
and on pull requests, with no `paths:` filter.

The reason recorded as decisive was reason 3 in the workflow section: a check
placed after clippy never runs when clippy is red, which would be a sixth variant
of a gate that cannot fire, deliberately constructed. Do not relitigate the
placement on convenience grounds.

The console's whole differentiator is that every fact is generated from the
source tree and gated against drift. Today that gate only fires when somebody
builds the website. Add an RPC method to `crates/node/src/rpc.rs`, commit, push,
and nothing anywhere goes red: the console is silently stale until the next
website build, which might be weeks later and might be done by someone who
assumes a green build means a correct page.

CI closes that. The gate stops being a property of the build and becomes a
property of the repository.

### What it runs

Two commands, and deliberately not `npm run build`:

    node website/scripts/generate-console-data.mjs --check
    node website/scripts/generate-console-html.mjs --check

`--check` regenerates from the tree and byte-compares against the committed
artifacts, so it fails when the repository and the published page disagree.

### The finding that decides the cost: it needs no dependencies

`generate-console-data.mjs`, `generate-console-html.mjs`, `tokenise.mjs` and
`freeze-console.mjs` import **nothing outside `node:` builtins** and one relative
import of each other. Verified by grepping every static import and every dynamic
`import(` and `require(` in all four: there are none.

So the job needs **no `npm ci`, no `node_modules`, no lockfile install**. It is a
checkout, `actions/setup-node`, and two `node` invocations. Estimated 20 to 30
seconds wall clock. On a public repository GitHub Actions is free; on a private
one this is about a minute a push against the free monthly allowance. The cost is
not the consideration.

It also means the job cannot break because of an npm registry outage or a
transitive dependency change, which matters for a check whose whole job is to be
trustworthy.

### It needs its own workflow, not a step in `ci.yml`

`ci.yml` today is: checkout, Rust toolchain, LLVM and Clang via apt,
`cargo fmt`, `cargo clippy -D warnings`, `cargo test --all --all-features`,
`cargo install cargo-deny --locked`, `cargo deny check licenses`. It triggers on
push to every branch and on pull requests.

Three reasons not to add a step to it, the third being the one that should settle
it:

1. **No shared setup.** The drift check needs no Rust, no LLVM, no cargo. Bolting
   it on means a 20-second check reports only after ten-plus minutes of Rust
   build and a `cargo install`.
2. **Failure attribution.** "console drift" red and "rust tests" red are
   different problems for different owners. One job conflates them.
3. **Steps run in order and stop at the first failure.** As a step after clippy,
   a clippy failure means the drift check never runs at all. That is precisely
   the "a gate that never runs" failure mode this project has now found five
   variants of, and building a new one on purpose would be indefensible.

### Triggers, and the one thing not to do

    on:
      push:
        branches: [main]
      pull_request:

**Do not add a `paths:` filter.** The instinct is to scope it to
`crates/**`, `sdk/**`, `docs/**`, `README.md`, `website/**`, and that instinct is
wrong here. The generators read a specific and growing set of files, and a filter
that misses one produces exactly the silent staleness the job exists to prevent.
The job is 20 seconds. Running it always costs less than an incomplete filter
costs once.

### What it catches, and what it does not

Catches: a new, renamed or removed RPC method (the four-way name-set gate); a
moved dispatch arm, which shifts published source-link line numbers; a changed
constant value; an edited reference table; a newly emitted error code; and a
document defect being FIXED, which makes a `KNOWN_DRIFT` exception stale and
names it for deletion.

Does not catch: anything needing the built page or the browser, the TypeScript
typecheck, or the vitest suite. All of those need `npm ci` and are a second,
slower job if wanted. `freeze-console.mjs --check` is dependency-free and could
join the fast job, though it adds little while `console-html --check` is green,
since the pages cannot drift from the data without that failing first.

### `repo-stats` stays OUT of CI. DECIDED 2026-09-03.

A policy decision rather than a technical one, and it went the way the reasoning
below points: `repo-stats` publishes marketing numbers on the main site, so it is
not the console's to gate. **Revisit when the marketing site work resumes**, not
before, and not as part of standing up the console job.

The reasoning, kept because the revisit will need it:

`generate-repo-stats.mjs --check` is red locally right now, and it is red for a
reason that **does not exist in CI**: it walks the working tree, and this working
tree carries uncommitted chain work. A CI checkout is committed state by
definition, so in CI the check measures exactly what it should.

That makes including it a real policy choice with a real consequence: every
commit that changes `crates/` would have to regenerate and commit
`repo-stats.generated.json`, or main goes red. It publishes marketing numbers on
the main site and it is the operator's file, so my recommendation is to start
with the two console checks only and decide `repo-stats` separately, on its own
merits, once the fast job has been green for a while. That is the decision
recorded above.

### The same point applies to the console checks, and it is the strongest argument for CI

`console-data --check` is red in the working tree as this is written, for the
same reason: uncommitted chain work has moved a line number the console
publishes. See the SHA-pinning note in section 3 for the measurement.

That is not a defect in the data. It is the check comparing a committed artifact
against an UNCOMMITTED tree, which is a comparison it should never have to make.
In CI the two sides are both committed by construction, so the check finally
measures the thing it was written to measure: does the published console agree
with the repository as it actually exists for everyone else?

This inverts how the local red should be read. Locally it means "somebody is
mid-edit". In CI it would mean "main is stale", which is the only version of that
signal worth acting on, and the only version that is actionable by whoever pushed.

---

## 12. The rail teaches ten sections; the navigation has eight destinations

**DECIDED 2026-09-03: option C, grouping only. Build it at Gate C6.**

The decision, so a future session implements it rather than reopening it:
**grouping only, spacing only, no renumbering, no nesting, no new component.**
Options A and B below are recorded as rejected with their reasons, not as live
alternatives. The ten section numbers stay, the eight pages stay, and every `h2`
keeps the number it has.

The rail lists ten numbered entries. Two pairs share a page: `01 connect` and
`02 first call` both resolve to `/console.html`, and `08 network parameters` and
`09 known gaps` both resolve to `/console/network.html`. That split was approved
on content grounds and is right. The dissonance is that the numbering implies ten
units of navigation when there are eight.

Worth stating precisely, because it bounds the severity: the pairing IS visible
when the reader is on the owning page, where both entries carry `aria-current`
and both are same-page fragments. From every other page the two are ordinary
cross-page links and nothing distinguishes them from separate destinations. So
this is a clarity defect, not a broken interaction: clicking `02` from the SDKs
page lands correctly, on `/console.html#first-call`.

### Option A: ten pages, one per numbered section

Rejected, and the reasons are concrete rather than aesthetic.

It undoes a content decision made for the five-minute goal. Connect and first
call sit together so the endpoint and a runnable curl are in one scroll;
splitting them makes the first page a four-row table and one command, then a
click to reach the first real call. Known gaps is largely about parameters
(retention, pruning), so that pair is a topic, not an accident.

It also adds two pages to the cross-page reference surface, and that surface is
measured: the eight-way split produced eight cross-reference defects, which is
the single largest class this gate had to close. Paying that cost again to
resolve a numbering artifact is the tail wagging the dog.

### Option B: an eight-entry rail with sub-entries

Rejected. It renumbers the rail to 01 to 08 while every `h2` on every page reads
`01 connect` through `10 verify it yourself`, so the rail and the headings would
state different numbers for the same thing, which is strictly worse than today.
Renumbering the headings to match discards the ten-step reading order the section
numbering exists to encode. It also introduces a nesting level into a flat rail.

### Option C, THE DECISION: group the rail by destination

Keep ten numbers, keep eight pages, keep every heading as it is, and group the
rail so the entries that share a page read as one destination. The minimum
version is spacing only: emit the ten items in their four page groups, with the
existing rhythm inside a group and one step more between groups. No new colour,
no new type size, no nesting, no new component.

It is the only option that changes neither the page structure, nor the
numbering, nor a content decision already made on its merits. It resolves the
mismatch by making the number stop implying a destination, which is the actual
conflict: the numbers are doing two jobs, signalling reading order and implying a
unit of navigation, and only the second one is wrong.

Severity is low and it is a design change, so it belongs with the rest of the
navigation work at Gate C6 rather than inside a correctness gate. It is decided,
not merely recorded: build it at C6 to the constraints at the head of this
section.
