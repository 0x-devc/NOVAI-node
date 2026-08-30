# Gate C4, Phase 3 verification

Run 2026-08-30 against the rebuilt console, frozen to `website/snapshots/` first so
the artifact attacked is the artifact that was built.

**Agent 4, the falsifier: 341 claims attacked, 13 falsified.**

Its previous run, against the C3 page, attacked 241 and falsified 13. This one
attacked 41 percent more and broke thirteen again, on a page that had just had
fifteen defect classes closed and seven new gates added.

---

## My judgement on whether the attack was real

Yes, and by a harder measure than last time.

**It re-found none of the fifteen closed defects as still broken.** It went
further and actively confirmed several of the fixes against the running node:
the `-32600` correction is right, the null-answer badge set is complete in both
directions (it enumerated all six `Value::Null` sites and matched them to the
four badged methods), all six record anchors resolve to rendered shapes, and all
88 rows of the names index name a string genuinely rendered on the page they
point at.

**It broke thirteen new things, and I verified the five most consequential
myself before accepting any of them.** Two are defects I introduced in this
gate. Three are holes in gates I wrote. One is a defect that a Phase 1 agent had
examined and cleared.

**It found the class its previous run missed.** The cross-reference mandate was
added because that run caught enumerable absence and missed dereferenced
absence. This run's second-largest finding is a whole section of broken
self-references, which is exactly that class.

**Its instruments were validated before use, and it said so when one was weak.**
It self-tested the link checker on a fabricated anchor and a real one, injected
two fake rows into the names verifier to confirm it catches them, and ran the
generator's own `backtickedIdents` on a string it must catch and on the real
clause. It then reported that its mechanical contradiction scan produced only
false positives and **could not structurally have found the largest count
defect**, and that the finding came from reading rather than from the scan. An
agent that reports its own instrument as weak is not performing.

**It withdrew an attack rather than inflate the count.** The console normalises
a U+2212 in the reference to an ASCII hyphen, which alters a quotation. It
checked, found the substitution is printed on every generator run, and declined
to call it a falsification while still recording the asymmetry as worth a
decision.

---

## The thirteen, verified where it matters

### 1. The canonical entity id derivation is published truncated

Rendered: ``the canonical entity id, `blake3("NOVAI_AI_ENTITY_ID_V1" \``, ending
on a dangling backslash.

`docs/RPC_REFERENCE.md:400` reads
``blake3("NOVAI_AI_ENTITY_ID_V1" \|\| code_hash \|\| creator)``. The markdown
table parser splits rows on `|` and the doc escapes its pipes as `\|`, so the
parser cut the cell at the first escaped pipe and dropped the rest.

**Verified myself: `code_hash || creator` occurs zero times across the entire
console.** `entity_id` is the required parameter of fourteen of the twenty-nine
methods and the page gives no way to compute one. This is the same class as the
undefined record shapes, found in a different mechanism, and it is the most
consequential single finding of the run.

### 2. `novai_getSignalsByHeight` publishes an error it cannot emit

Rendered under a method whose only parameter is `height`:
`-32602 | end_height - start_height > 10000 (range queries)`.

**Verified myself:** `handle_get_signals_by_height` emits `-32602` and `-32002`
and contains no reference to `MAX_SIGNAL_QUERY_RANGE` at all.

This is the identical defect the page now carries as a disclosed, corrected,
badged exception for `novai_listVkRegistrations`, appearing here undisclosed and
uncorrected. See the gate holes below for why the check I built cannot see it.

### 3. `novai_getLatestBlock` claims only global errors and emits `-32002` twice

**Verified myself:** `crates/node/src/rpc.rs`, `fn handle_get_latest_block` at
:3600, with `code: -32002` at :3611 and :3618. The two sibling block methods
both document `-32002`; only this one claims immunity, and it is the method
every integration calls first.

### 4. The seller cap sentence is false, and a Phase 1 agent cleared it

Rendered on `listSlasBySeller`: "Bounded internally by the per-buyer cap
(= 8 in v1)."

**Verified myself,** `crates/ai_entities/src/memory.rs:158-163`: *"The cap is per
BUYER (memory-object owner). Sellers are not capped in v1: they appear in
arbitrarily many SLAs but never own the underlying memory object."*

Worth recording precisely: in Phase 1, Agent 2 examined this exact sentence,
read the rustdoc at `rpc.rs:2475-2480`, and reported it "CORRECT but only by
luck ... No gate could have told you that; I read the rustdoc." It read one
rustdoc and not the other. The falsifier read the constant's own declaration and
got the opposite answer. This is the cross-validation working, and it is why an
agent's report is not a verdict.

### 5. "Five discrepancies are known" is still hardcoded, and there are nine

`scripts/generate-console-html.mjs:248`, rendered on the network page.

**This is my incomplete fix.** I moved the intro paragraphs into a generated
region so their counts derive, and missed the same hardcoded five inside a
`PROSE` string. The page now states both numbers: `console.txt` says "There are
9 of those today" and `console__network.txt:130` says "Five discrepancies are
known", on the same surface. My `assertProseIsAllUsed` gate checks that every
`PROSE` key is rendered; it does not check that a number inside one is derived.

### 6. The AI entities page promises citations it does not give

Rendered: "with every constant cited to its declaration." Measured: zero
`file:line` citations and zero source links on that page, against 33 on the
reference page. Every constant it asserts is nonetheless true, each verified
independently. The facts hold; the promise about the page does not.

### 7. A cross-page reference to a conversion the target refuses to make

Entities: "The lock is counted in blocks, not in time; the cadence in the network
section converts it." Network: "Retention is published in blocks and never
converted to wall-clock time ... Divide by the rate you measure yourself."

The two sentences are in direct opposition, and the second is a settled decision
of this project.

### 8. The names index harvested a constant out of a filename

`RPC_REFERENCE | constant | get started`. It occurs zero times in `crates/` or
`sdk/`; on the landing page it appears once, as a fragment of the path
`docs/RPC_REFERENCE.md` in the provenance line. My `SCREAMING_SNAKE` scanner
read it out of a filename. "get started" is also a page label that appears in no
navigation on any page.

### 9. `names.html` ships four dead anchors marked as the current page

`#connect` and `#first-call`, in both navs, with `aria-current="true"`, and
neither id exists on that page.

**My bug, and the mechanism is worth stating:** the two generated find surfaces
take their shell from the already-rendered landing page, so they inherit the
landing page's navigation verbatim, including its same-page fragments and its
current-page marks. Every other page emits `/console.html#connect`.

### 10. Five "see X below" references on the reference page point off it

`// 0..=15 (see Memory types below)`, `// 0..=22 (see Signal types below)` three
times, and `// bitfield (see Capabilities)`. All three tables live on the
entities page, and there are zero links to them from the reference page.

These read correctly on one page and broke the moment the page split, which is
precisely the cost the split was supposed to be designed against. They survive
on `all.html`, which is why a whole-surface check passes and a per-page one does
not.

### 11. "The upgrade transaction changes the code hash and nothing else"

`crates/execution/src/lib.rs:9706-9709` mutates `last_active_at` as well, and
:9735 writes an `UpgradeSummary`. The console's own `getAiEntity` result shape
publishes `upgrade_count` and `last_upgrade_height` as fields that change on
upgrade, so the page contradicts itself across two pages.

### 12. Every measured number in the tokeniser's own rationale is stale

`scripts/tokenise.mjs` argues for a hand-written tokeniser from "84 code blocks,
51 JSON-shaped, 31 shell" and "JSON.parse fails on 29 of the 51". Measured today:
87 blocks, 55 schema, 30 shell, and 34 of the 55 fail to parse.

**Mine, and written this session.** The argument is unaffected and gets stronger,
but a comment that cites measurements is a published number, and I did not
re-measure after my own changes moved it. This is the exact failure the whole
gate is about, committed by me inside the fix for it.

### 13. The chart caption's definite articles are wrong

"The most expensive, entityUpgrade ... 50 times the cheapest, transfer." Three
types tie at 5,000 and two tie at 100. The 50x ratio is correct; "the most" and
"the cheapest" are artefacts of a sort order.

---

## Gates that cannot fail: three holes, all in gates I wrote or extended

**(a) The inherited-meaning gate is blind to category-common error tables.**
Both `assertInheritedMeaningIsTrue` check (b) and the
`inheritedForeignFields` measurement open with `if (!m.errors?.resolvedFrom)
continue;`. A category-common table is built as
`{ kind: "categoryCommon", list, from }` with **no `resolvedFrom` key at all**,
so all three Signal methods inherit their errors by a route the gate cannot see.
That is the mechanism behind finding 2.

**(b) `backtickedIdents` cannot parse the clause that carries the defect.** It
requires the whole backtick span to be a bare identifier, and the doc wraps the
entire expression: `` `end_height - start_height > 10000` ``. The agent ran the
generator's own function and showed it returns `[]` on the real clause and
`["id"]` on one it must catch. So even with (a) fixed, the check finds nothing.
Two independent reasons the same gate reports clean.

**(c) `assertLossless` is tautological on the `lang: "none"` path.** That path
builds a single token `{cls: "plain", text: src}` and the assertion then compares
`tokens.join("")` to `src` and filters for `!t.cls`. Both are true by
construction. The two hand-authored blocks that take this path are asserted and
unchecked.

All three are real. (c) is the exact "a check reads a value back from the same
parse that produced it" pattern this project keeps a note about.

---

## What it could not verify, stated rather than rounded up

Registry state for PyPI, crates.io and npm, with no reachable registry. The claim
that the SDK source is unchanged since release, which depends on that date. The
pruned-transaction `-32002` band, which the page itself says is not demonstrable
from outside. The capture timestamps on the first-call outputs, whose blocks are
now far below the prune horizon and cannot be re-fetched, though it did confirm
the three heights are mutually parent-hash consistent.

## A separate finding worth acting on

`check-snapshot-freshness.mjs` is **red**: the committed chain snapshot records
height 4,354,701 against a live tip of ~5,433,006, a gap of 1,078,305 against a
50,000 block retention window, or about 21 retention windows stale. It sits in
`predeploy`, which refreshes before checking, so it self-heals on the next
deploy. Worth knowing before one.

---

# Agent 5: accessibility and responsive

**Agent 5 died on a session limit partway through, before its browser pass, and
produced no report.** I ran its mandate myself rather than report a gap. Every
measurement below is mine, and each says how it was taken.

## Viewports: 40 of 40 clean

Driven through CDP `Emulation.setDeviceMetricsOverride` against
`npx vite preview`, never by sizing the window, because Chrome on macOS clamps
its window to about 500px and a `--window-size=360` run reports overflow that is
not real. `window.innerWidth` was read back on every run and matched the
requested width exactly.

Ten pages by four widths, comparing `document.documentElement.scrollWidth`
against `window.innerWidth`:

| width | document overflow | main width | method panes | rail / chips |
|---|---|---|---|---|
| 360 | none, 10/10 | 360 | stacked (`display: block`) | hidden / shown |
| 768 | none, 10/10 | 762 | stacked | hidden / shown |
| 1440 | none, 10/10 | 1231 | **two columns** (`display: grid`) | shown / hidden |
| 2560 | none, 10/10 | **1523** | two columns | shown / hidden |

Three things that were designed and are now measured rather than assumed:

- **Nothing overflows the document at any width**, including the 195 kB
  reference page and the 266 kB all-in-one page. Wide tables and code fences
  scroll inside their own containers, which is what they were built to do.
- **The two-pane layout switches where it should.** `display` is `block` at 360
  and 768 and `grid` at 1440 and 2560, so the grid is inert below the breakpoint
  and the DOM order is the reading order at every width.
- **The 1760px cap holds.** At 2560 the content measures 1523px rather than
  filling, which is the 110rem container minus the rail. That matches the
  unanimous behaviour of every reference site measured: Stripe caps at 1760,
  ethereum.org at 1536, Solana at 1440, and none of the eleven fills a 2560
  viewport.

## Keyboard: clean, and my first instrument was wrong about it

Walked the real tab order with real CDP key events on three pages.

- **Skip link is the first tab stop** on every page, becomes visible on focus,
  and its target resolves.
- **31 to 40 tab stops walked, `:focus-visible` true on every one, and zero
  stops with no visible indicator.**
- **All seven copy buttons on the landing page are reachable by Tab and reach
  `opacity: 1` when focused.** This was the specific risk in my own design: the
  button is `opacity: 0` until hover, and a control a keyboard user can focus
  but not see is a real defect.

**My first measurement said the opposite and was wrong twice, which is worth
recording.** Using `element.focus()` from JavaScript, I measured 12 of 25
elements with no focus indicator and the copy button stuck at opacity 0.
Programmatic focus does not reliably engage `:focus-visible`, so the probe was
measuring the wrong state. Checking the built CSS showed
`.console-copy:focus-visible{opacity:1}` and `.console-rail:focus-visible{outline:...}`
both present and correct. Re-running with real Tab keys gave 0 of 40 without an
indicator. A second sample at 40ms then read opacity 0.277, which was the 120ms
transition still running; sampling after it settles reads 1.

Two wrong readings in a row on the same control, both from the instrument rather
than the page.

## Semantics

- **Heading order is clean on all ten pages.** No level is skipped anywhere,
  including `all.html`, which concatenates 150 headings from ten sections.
  Exactly one `h1` per page.
- **One `<main>` per page**, all navs labelled, none unlabelled.
- **`aria-current` is exactly right on all eight real pages**, on the owning
  page's own sections and on nothing else.
- **The duplicate "Sections" nav label is safe by construction, not by luck.**
  Every page carries two navs with that label, the rail and the chips, which
  would be an ambiguity for anyone listing landmarks. They are mutually
  exclusive: `.hidden{display:none}` below 1024px and
  `@media(min-width:1024px){.lg\:hidden{display:none}}` above it, both confirmed
  in the built CSS and both confirmed live at all four widths. Exactly one is in
  the accessibility tree at any viewport, so no one ever hears two.

## Defects

1. **`names.html` ships four dead `#connect` / `#first-call` anchors marked
   `aria-current="true"`.** Independently confirmed; it is the same defect the
   falsifier found, reached by a different route. The generated find surfaces
   inherit their shell, and therefore their navigation, from the rendered
   landing page.
2. **The skip link moves the scroll but not the focus.** Activating it sets
   `location.hash` to `#main`, and `document.activeElement` stays `BODY`,
   because `<main id="main">` carries no `tabindex="-1"`. A screen reader user
   who uses it continues reading from where they were rather than from the
   content. One attribute fixes it.

## Judgement calls, not defects

- Seven of the ten pages carry zero copy buttons, because they carry zero code
  blocks: all 87 fences live on the landing page and the reference page. Worth
  noticing that the SDKs page has no runnable example, but that is the standing
  ruling, not an oversight, since no quickstart has been verified yet and the
  rule is to publish none that has not been run.
- `all.html` marks `#connect` and `#first-call` with `aria-current`. Those
  anchors do resolve there, so it is not dangling, but calling them "current" on
  an aggregate page is loose.

## What I could not test

Real screen readers, real mobile hardware, the production Cloudflare
environment, and any assistive technology beyond the accessibility tree Chrome
exposes. Contrast was verified through the calibrated audit and its new syntax
token rows, which compute from declared tokens; I did not re-derive painted
pixel values from screenshots.
