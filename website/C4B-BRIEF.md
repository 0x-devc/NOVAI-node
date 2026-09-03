# Gate C4b: the thirteen

**CLOSED 2026-09-04.** All thirteen are fixed, plus a fourteenth found at plan
time and six further instances found by sweeping the classes rather than the
list. Landed in `169fc0e`, `9e5054b`, `17ff864` and `ad58a1f`, none pushed.

**The current work list is `C4C-BRIEF.md`, and the evidence for it is
`C4B-VERIFICATION.md`.** Read those instead of this file. This one is kept
because its reasoning explains why several fixes took the shape they did, and
because the run 2 numbers in it are the baseline the convergence question is
measured against.

One correction to leave visible: the P2 recommendation below, to fix the
document or carry a tenth exception, was WRONG. The document was not wrong. It
scoped the range row with "(range queries)" and the console dropped the
qualifier when flattening a category-common table. The fix was a rule about
inheritance, not a document edit. That is the third of three times in this gate
a prescribed fix turned out to be wrong when checked against source.

Written 2026-08-30, at the close of Gate C4.

**Read this with `PHASE3-REPORTS.md`.** That file carries the evidence: what was
attacked, how each finding was verified, and which instruments were validated
before use. This file is the work list.

Gate C4 landed as two commits, `c4a9f3b` (correctness) and `2454da1`
(structure). Neither is pushed. The adversarial pass that ran against the result
attacked 341 claims and falsified 13. **Three of the thirteen are defects
introduced by C4 itself, and three separate gate holes are in gates C4 wrote.**
That is the honest state: the gate closed fifteen defect classes and opened
three, and its own verification found them.

Nothing here is public. The console carries `noindex` and is not linked from the
site, so this is debt, not an incident.

---

## The four priority items

Do these in this order. Everything else in the list can follow.

### P1. The canonical entity id derivation is published truncated

Rendered on the `novai_getAiEntity` params table:

    the canonical entity id, `blake3("NOVAI_AI_ENTITY_ID_V1" \

It ends on a dangling backslash. `docs/RPC_REFERENCE.md:400` reads
``blake3("NOVAI_AI_ENTITY_ID_V1" \|\| code_hash \|\| creator)``. The markdown
table parser splits rows on `|`, the document escapes its pipes as `\|`, and the
parser cut the cell at the first one.

**Measured: `code_hash || creator` occurs zero times across the entire console.**
`entity_id` is the required parameter of fourteen of the twenty-nine methods, so
the page gives a developer no way to compute one.

Fix in `parseTable` in `scripts/generate-console-data.mjs`: split on unescaped
pipes only, and unescape `\|` in the resulting cells. **Then gate it**: no
rendered cell may end on a lone backslash, and no cell may contain an unbalanced
backtick. Both conditions are cheap and both would have caught this.

This is the same class as the record shapes C4 fixed: content the parser silently
dropped. It was found in a different mechanism, which is the point.

### P2. An error clause published on three methods that cannot emit it, behind two blind gates

`novai_getSignalsByHeight` publishes `-32602 | end_height - start_height > 10000
(range queries)` under a params table whose only row is `height`. Verified:
`handle_get_signals_by_height` emits `-32602` and `-32002` and contains no
reference to `MAX_SIGNAL_QUERY_RANGE`.

This is the identical defect the console now carries as a disclosed, corrected,
badged exception for `novai_listVkRegistrations`. Here it is undisclosed. Two
independent reasons the gate cannot see it, both in `generate-console-data.mjs`:

- **Hole A.** `assertInheritedMeaningIsTrue` check (b) and the
  `inheritedForeignFields` measurement both open with
  `if (!m.errors?.resolvedFrom) continue;`. A category-common errors table is
  built as `{ kind: "categoryCommon", list, from }` with **no `resolvedFrom`
  key**, so all three Signal methods inherit by a route neither check sees.
- **Hole B.** `backtickedIdents` requires the whole backtick span to be a bare
  identifier, and the document wraps the entire expression:
  `` `end_height - start_height > 10000` ``. It returns `[]` on the real clause.
  So fixing Hole A alone changes nothing.

Fix both, then decide whether the clause is a document defect to carry as a tenth
exception or one to fix at source. Prefer fixing the document: an exception is
debt and this one has no reason to exist.

### P3. The split broke eight cross-references and left two promises unkept

These read correctly on one page and broke the moment it became eight. They
survive on `all.html`, which is why a whole-surface check passes and a per-page
one does not.

| Promise | Made on | Target |
|---|---|---|
| "see Signal types below" (x3) | rpc | table is on entities, unlinked |
| "see Memory types below" | rpc | table is on entities, unlinked |
| "see Capabilities" | rpc | table is on entities, unlinked |
| "the cadence in the network section converts it" | entities | network publishes no cadence and states it will never convert |
| "with every constant cited to its declaration" | entities | zero citations and zero source links on that page |

The last two are worse than dangling links. The cadence one points at a settled
decision of this project and asserts its opposite. The citation one is a promise
the page makes about itself in a sentence, which is the class the C4 falsifier
mandate was rewritten to catch.

**Gate it.** Every "see X" and "below" reference must resolve to an anchor that
exists on the page making it, or be rewritten as a cross-page link. This is a
scan over rendered text plus the id set of each page, and it is the single
highest-value gate left unbuilt.

### P4. Two C4 fixes that were only half applied

- **"Five discrepancies are known" is still hardcoded** at
  `scripts/generate-console-html.mjs:248`, inside a `PROSE` string, rendered on
  the network page. There are nine. The same surface says "There are 9 of those
  today". C4 moved the intro counts into a generated region and missed this one.
  The `assertProseIsAllUsed` gate checks that every `PROSE` key is rendered; it
  does not check that a number inside one is derived. **Extend it**: no digit or
  number-word in `PROSE` that also exists as a derived value.
- **`names.html` ships four dead `#connect` and `#first-call` anchors marked
  `aria-current="true"`.** The two generated find surfaces take their shell from
  the already-rendered landing page, so they inherit its navigation verbatim,
  including its same-page fragments and its current-page marks. Render the nav
  for the find surfaces with their own page identity.

---

## The remaining nine

Ranked by consequence. Evidence for each is in `PHASE3-REPORTS.md`.

5. **`novai_getLatestBlock` claims "only the global ones" and emits `-32002`
   twice**, at `crates/node/src/rpc.rs:3611` and `:3618`. Its two sibling block
   methods both document it. This is the method every integration calls first.
6. **The seller cap sentence is false.** `listSlasBySeller` publishes "Bounded
   internally by the per-buyer cap (= 8 in v1)".
   `crates/ai_entities/src/memory.rs:158-163` says the opposite in as many
   words: *"Sellers are not capped in v1."* A client sizing a buffer on a
   guarantee of eight silently truncates. Note for the record that a Phase 1
   agent examined this exact sentence and cleared it, having read a different
   rustdoc; an agent's report is not a verdict.
7. **"The upgrade transaction changes the code hash and nothing else."**
   `crates/execution/src/lib.rs:9706-9709` also mutates `last_active_at`, and
   `:9735` writes an `UpgradeSummary`. The console's own `getAiEntity` result
   shape publishes `upgrade_count` and `last_upgrade_height` as upgrade-driven
   fields, so the page contradicts itself across two pages.
8. **The names index harvested a constant out of a filename.**
   `RPC_REFERENCE | constant | get started`. It occurs zero times in `crates/`
   or `sdk/`; on the landing page it is a fragment of the path
   `docs/RPC_REFERENCE.md` in the provenance line. The `SCREAMING_SNAKE` scanner
   in `renderNamesPage` must exclude matches inside a path or a filename. "get
   started" is also a page label that appears in no navigation.
9. **The tokeniser's own rationale cites stale measurements.**
   `scripts/tokenise.mjs` argues from "84 code blocks, 51 JSON-shaped, 31 shell"
   and "JSON.parse fails on 29 of the 51". Measured after C4: 87 blocks, 55
   schema, 30 shell, 34 failing to parse. The argument is unaffected and gets
   stronger. Written during C4, made stale by C4, and not re-measured. A comment
   that cites measurements is a published number.
10. **`assertLossless` is tautological on the `lang: "none"` path** (gate hole
    C). That path builds one token `{cls: "plain", text: src}`, and the assertion
    then compares `tokens.join("")` to `src` and filters for `!t.cls`. Both true
    by construction. The two hand-authored blocks that take it are asserted and
    unchecked. Either give them a real tokeniser pass or drop the claim for them.
11. **The chart caption's definite articles are wrong.** "The most expensive,
    entityUpgrade ... 50 times the cheapest, transfer." Three types tie at 5,000
    and two tie at 100. The 50x ratio is correct; the articles are artefacts of a
    sort order.
12. **The skip link moves the scroll but not the focus.** `<main id="main">`
    carries no `tabindex="-1"`, so `document.activeElement` stays `BODY` after
    activation and a screen reader user resumes where they were. One attribute.
13. **`all.html` marks `#connect` and `#first-call` with `aria-current`.** Those
    anchors resolve there, so it is not dangling, but calling them "current" on
    an aggregate page is loose. Lower priority than P4's dead anchors and the
    same root cause.

---

## One thing that is not a defect but needs a decision

The console normalises the U+2212 minus in `docs/RPC_REFERENCE.md:626` to an
ASCII hyphen, which alters a quotation of the reference. The substitution is
**printed on every generator run**, one line per code point, so it is declared
rather than silent. But it is declared to the builder and invisible to the
reader, and C4 removed `rewriteForeignFields` for exactly that asymmetry.
Decide it rather than let it stand by accident.

---

## Where to start

1. Read `PHASE3-REPORTS.md` for the evidence and the instrument notes.
2. `website/HANDOFF.md` section 3 for the gate ladder and the three red gates.
3. `node scripts/freeze-console.mjs` regenerates the ten frozen snapshots in
   `website/snapshots/`. Attack those rather than the HTML: every defect this
   project has shipped was correct in the data and wrong on the page.
4. Every new gate must be proven by injecting a violation and confirming the
   probe was seen. Three separate scans in C4 reported clean because they matched
   nothing, and each was caught only by feeding it something it had to catch.
