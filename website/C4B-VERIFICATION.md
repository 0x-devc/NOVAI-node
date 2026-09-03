# Gate C4b, verification

Run 2026-09-03 and 2026-09-04 against the console as it stands at `ad58a1f`,
frozen to `website/snapshots/` first so the artifact attacked is the artifact
that was built (`freeze-console --check`: ok, 10 snapshots match the pages).

Read this with `C4C-BRIEF.md`, which is the work list. This file is the evidence.

---

## The headline: the prediction failed and the registered criterion was met

**226 claims attacked, 15 falsified.**

**Decomposition: (a) pre-existing 8, (b) introduced by C4b 6, (c) gate hole 1.**

The prediction registered in the C4b plan, before the run, was **350 to 400
attacked, 4 to 7 falsified, of which 0 to 2 class (b)**. All three components
missed: fewer claims attacked, more than double the falsifications, and three
times the upper bound on self-inflicted ones.

The falsifying outcome was stated explicitly in advance: **ten or more falsified
means the constant hypothesis wins.** Fifteen is not a near miss.

| Run | Page | Attacked | Falsified | Rate |
|---|---|---|---|---|
| 1 | C3 | 241 | 13 | 5.4% |
| 2 | C4 | 341 | 13 | 3.8% |
| 3 | C4b | 226 | 15 | **6.6%** |

The count is flat to rising and the rate ROSE. Three gates, three correctness
passes, no convergence.

---

## What the number means, stated rather than explained away

**Nearly half of run 3's findings are damage this gate caused.** Seven of the
fifteen are class (b) or (c), and every one of those seven was introduced by
C4b itself. So the instrument is substantially measuring change volume rather
than page quality, which is exactly the reading the registration named in
advance as the honest one if class (b) dominated.

**But the eight class (a) findings are not change volume, and they are not
random.** They cluster, hard, and the cluster is the useful result:

> Runs 1 and 2 audited claims about the CHAIN exhaustively and claims about the
> PAGE'S OWN RUNTIME not at all.

Everything sourced from the tree held under a third attack. Run 3 re-verified
all 70 published `blob/main` source links against committed source, every
enumeration (29 methods, 23 signal types, 16 memory object types, 11
discriminants, 13 error codes, 4 HTTP rejections, 6 limits, 11 exceptions, 88
names), every fee and every payload arithmetic, the whole canonical encoding
table, and the C4b headline fix (`blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash ||
creator)` against `AiEntity::compute_id`). None of it broke.

What broke was the page describing behaviour the built artifact does not have.

**That is the conclusion, and it is not "audit harder".** Generate-then-audit
structurally cannot reach it: no amount of cross-referencing the source tree
proves the BUNDLE does what the PROSE says. The console needs a check of a
different kind before it is announced, one that ties every present-tense
operational claim to an assertion against the built artifact, the same way every
number is now tied to a derived value.

---

## The fifteen

Fourteen of the fifteen were verified independently at HEAD rather than taken
from the agent's report. Where a finding is agent-measured only, it says so.

### Class (a): pre-existing, and missed by both earlier runs

**F1 to F4. The two panels the console showcases do not exist.** This is the
largest finding of the run and the most serious defect the console has carried.

Published, in the present tense:

- `console/verify.html`: "This panel calls the live chain and re-derives a
  consensus property: it walks consecutive block headers and checks that each
  one's parent hash is the previous block's hash."
- `console/verify.html`: "This panel needs JavaScript. With it disabled the page
  still renders in full; only this check is unavailable." (implies it works with
  JavaScript on)
- `console.html`: "Live height and cadence load here when JavaScript is
  available."
- `src/console/console-main.ts`, in a comment: "From gate C4 it also mounts the
  two React islands, the live network panel and the verify panel."

Verified myself, statically, no browser required:

| Step | Evidence |
|---|---|
| The entry source | `src/console/console-main.ts` is two `import` statements for stylesheets and nothing else |
| The built entry | `dist/assets/console-main-*.js` is **29 bytes**: `import"./index-*.js";` |
| Mount reads | `grep -c data-island dist/assets/*.js` is **0** in every bundle, while both container elements exist in the markup |
| The component | `VerifyPanel` is imported only by its own file, `src/test/verify-panel.test.tsx`, and `src/dev/SpecimenApp.tsx`, and `vite.config.ts` deliberately excludes `specimen.html` from the production build |

`HANDOFF.md` schedules this as **Gate C5, not yet built**. The pages claim it
today. This breaks the project's own rule that public numbers derive from live
data or state a configuration fact, and it breaks it in the section that exists
to demonstrate the rule.

Debt rather than an incident only because nothing is reachable: the console
carries `noindex`, the site does not link to it, and production returns an SPA
catch-all for every path.

**F8. A "Measured at" citation points at the wrong null path.**
`novai_getLatestBlock` publishes "answers with a top-level null result rather
than an error when the record is not there, which for history includes anything
below the pruning horizon. Measured at `crates/node/src/rpc.rs:3606`."

Verified at HEAD: `:3605-3606` is `if height == 0 { return Ok(Value::Null); }`,
the empty-chain guard. The record-missing path is `:3631`. And the pruning
clause cannot apply to this method at all, because it reads
`index.committed_height`, the tip, which is never below the horizon. Boilerplate
applied to the one method it does not fit. The other three null citations are
correct.

**F9. The submitTransaction example is impossible by the console's own
arithmetic, and is not what the caption says it is.**

The caption reads "The `tx` value is a real ~300-byte hex blob". Verified: the
field is the literal placeholder `01<224 hex chars>`. So it is not real, it is
abridged, and three numbers that should agree do not:

| Source | Value |
|---|---|
| the placeholder itself | 226 hex chars = **113 bytes** |
| the console's own arithmetic (`TX_V1_OVERHEAD` 149 + `payload_len` 83) | **232 bytes** = 464 hex chars |
| the caption | **~300 bytes** |

Inherited verbatim from `docs/RPC_REFERENCE.md` and not carried as a drift
exception. The receipt is otherwise self-consistent (`payload_len` 83 matches
`registerAiEntityWithKey`, `fee` 5000 matches its constant).

**F10. A published example response the node cannot emit.** Verified by field
comparison at HEAD: `struct AiEntityJson` has **20** fields and **zero**
`skip_serializing_if` attributes, so every field always serialises. The
published example shows **13**. The seven absent are `reputation_score`,
`total_transactions`, `reputation_events_count`, `stake_balance`,
`stake_locked_until`, `upgrade_count`, `last_upgrade_height`.

This lands on a cross-page claim: `console/entities.html` says "The effect is
checkable in one read: `novai_getAiEntity` returns `reputation_score` and
`reputation_events_count`." The console's only example of that method shows
neither of them.

**F13. A "Rejected unless" names one of three rejection gates.**
`console/entities.html`, signal type 10 `StakeWithdraw`: "Rejected unless
`stake_locked_until <= current_height`."

Verified at HEAD, `crates/execution/src/lib.rs`: the handler has three gates.
`StakeStillLocked`, `InsufficientStakeBalance`, and
`StakeWithdrawWouldUnderfundSlaCollateral` at `:7782`, which rejects a withdrawal
that would drop `stake_balance` below the summed collateral of open SLAs where
the entity is the seller. An SLA seller who satisfies the published condition is
still rejected.

### Class (b): introduced by C4b. All six are mine.

**F12. I published a fabricated justification.** In the sentence fixing the
citation promise I wrote that capability bits carry no link "because they come
from a bitflags declaration with no per-bit line to point at."

Verified with a positive control on the same grep before trusting the negative:
`bitflags` appears **zero** times in any `.rs` or `.toml` under `crates/` or
`sdk/`. `Capabilities` is a plain `pub struct` of `bool` fields, each on its own
documented line, and every bit is assigned on its own line in `to_byte` and
`from_byte`. `crates/ai_entities/src/lib.rs:170` literally reads
`submit_reputation_updates: (byte & (1 << 5)) != 0`.

There is no bitflags declaration and there ARE per-bit lines to point at. I
invented a mechanism to justify an exclusion, without checking, inside the gate
whose entire subject is not doing that. **This is the worst finding of the run
against my own work and it should be read before any other.**

**F11. The universal claim wrapped around a correct count is false.** "Every
constant above is cited to the line that declares it: 10 citations across 8
declarations." The count is right and gated. The universal is not: verified that
`DEFAULT_REPUTATION_SCORE` and `MAX_CATALOG_OFFERINGS` appear zero times on that
page, and `STAKE_LOCK_PERIOD` ("a deposit locks for 1,000 blocks") is uncited
while appearing in the console's own names index as a constant on that very page.

**F5. My new lead sentence contradicts the aggregate page 18 times.** Verified:
"the responses on this page were captured by running them" occurs once on
`console/all.html`, and "Example response from the reference ... not a current
reading" occurs **18** times on the same page. The sentence is true of the
landing page it was written for and false the moment `all.html` concatenates all
ten sections.

**F6. One of my five cross-reference notes describes text that is not there.**
Verified by walking each note back to its fence: four of five attach to comments
containing "below". The fifth attaches to `// bitfield (see Capabilities)`,
which contains no "below", while the note asserts 'The comments above say
"below"'.

**F7. My P2 fix broke a neighbouring claim.** Removing the range row from
`novai_getSignalsByHeight` was correct. "Shared by every method in Signal
methods" still renders on all three Signal methods, and is now false: verified
that `getSignalsByHeight` carries 2 error rows and the two range methods carry 3.
Before C4b all three were identical and the note was true.

**F14. The comment warning about rot is now the rotted one.**
`scripts/tokenise.mjs` says "These figures were stale within one gate of being
written ... and not re-measured after it." Every figure beside it is exact
(agent-measured, all six: 87 blocks, 55 schema, 30 shell, 2 hand-authored, 34 of
55 failing `JSON.parse`, and the shell blocks using exactly `curl` with `-s -X -H
-d`). C4b replaced the stale numbers AND added the staleness paragraph in the
same commit, so with the old numbers deleted the paragraph has no antecedent but
the correct ones. A reader is told to distrust data that is right.

### Class (c): a gate that cannot fail. Also mine, from commit 2.

**F15. One code-side fact is read from a comment.**
`generate-console-data.mjs`: `sellersAreUncapped` tests
`/Sellers are not capped/i` against `slaCapDoc`, which is the 900 characters
preceding the constant, that is, its rustdoc. The comment beside it claims the
fact is measured against the constant's own declaration. A declaration's doc
comment is not the declaration's behaviour.

Ten of the eleven exceptions pair a document fact with a CODE fact. This one
pairs a document fact with another comment, so if a seller cap were added and the
rustdoc left alone, the exception would keep holding and the console would keep
publishing "Size for an unbounded result set rather than for eight."

The conclusion it supports is nonetheless TRUE, verified from code rather than
from the comment: `MAX_SLAS_PER_ENTITY` has exactly one enforcement site,
`crates/execution/src/lib.rs:10542`, in the SLA create handler counting the
BUYER's own objects, and `handle_list_slas_by_seller` applies no cap at all. A
code-side measurement is available and cheap.

---

## Attacked and survived

Listed because a falsifier that reports only failures says nothing about
coverage. Each was attacked expecting it to break.

- **All 70 published `blob/main#L` links** resolve to the exact declaration or
  dispatch arm claimed, checked against committed source with `git show HEAD:`.
  All 29 method links land on their own `"novai_..." =>` match arm.
- **Every count and every enumeration**, including `openrpc.json` agreeing with
  the page in both directions.
- **Every fee**, all 11 `MIN_FEE_*` values, all three `*_FEE_BPS`,
  `TX_V1_OVERHEAD = 149`, and the canonical encoding table against
  `encode_tx_v1_unsigned` (little-endian envelope, big-endian payload internals,
  domain-tagged signing preimage, `txid = blake3(unsigned)`).
- **Every payload arithmetic**: all 16 tailed signal types sum correctly.
- **The C4b verbatim-quote fix**: `docs/RPC_REFERENCE.md:626` contains bytes
  `e2 88 92`, so U+2212 and `&#8722;` are correct; `rpc.rs:1241` contains a real
  U+2014, so `&#8212;` is correct.
- **Live falsification against the public endpoint**: missing `method` answers
  `-32700` not `-32600`; missing `jsonrpc` answers `-32700`; `"jsonrpc":"1.0"`
  answers `-32600`; height above tip answers `-32602` carrying the committed
  height; range over 10000 answers `-32602`. Every one matches what the page
  publishes, including the corrected `-32600` trigger.
- **The retention boundary re-measured live** at tip 6,884,291: 45,000 and 49,000
  back return blocks, 50,500 and beyond return `null`. The boundary is where
  `PRUNE_RETAIN_BLOCKS = 50,000` says it is.
- **Off-by-one probes**: the rate limiter allows exactly 100/s, concurrency
  exactly 64, and the range check accepts `end - start == 10000`, matching the
  published `<= 10000`.
- **All 435 internal fragments and 691 anchors resolve**, and all 88 names-index
  rows resolve to a page that really renders the name inside the named section.
- **Registry claims verified live**: PyPI `novai-sdk` is 0.1.0 and a single
  release; crates.io `novai-sdk` is 404; npm `novai-sdk-ts` is 404.
- **The published source links are correct against the REMOTE today.** Remote
  `main` is `98d2e52` and local HEAD is `ad58a1f`, and **zero** files differ
  between them under `crates/`, `sdk/`, `docs/` or `README.md`, because all four
  C4b commits touched only `website/`. This is the risk the SHA-pinning
  recommendation exists for, checked rather than assumed.

---

## Unverifiable, not rounded up to true

1. ~~**"Validators: 4"**~~ **RESOLVED TRUE 2026-09-04** by operator measurement
   of live process arguments: four validator processes, running since 08-30. No
   RPC method exposes the validator set, so this could not be reached from here,
   and the repository's genesis files say 1, 5 and 5, which is why the hand-set
   value with an excluded generator was the right call. The claim is true today.
2. **"the read path on this page is verified against the public endpoint"**.
   Still unverifiable, but the CAUSE is now known and is not a chain property:
   the operator's transaction generator is not running, which is why
   `getSignalsByType` returned zero signals over a 9,000 block window and why
   the example entity id answers `{"entity":null}`. The methods answer
   correctly; there is simply no data to exercise them.
   **That explanation must never appear on the console**, because it is a fact
   about one deployment rather than about the chain. See `NEEDS-OPERATOR.md`
   item 3c.
3. **The first-call capture heights**. Long pruned. Internally consistent (the
   curl block hash at 5,225,248 is the python parent hash at 5,225,249), which
   is the strongest check available.
4. **The 413, 429 and 503 bodies**. Source-verified, behaviour unmeasured: the
   public endpoint was deliberately not stressed.
5. **`ServiceDescriptor` category ranges**. No enforcement or allocation table
   found in `crates/`; possibly convention only.

---

## Instruments, validated before use

| Instrument | How it was validated | Result |
|---|---|---|
| Link and fragment checker | Four probes injected into the scan pass only: same-page bad fragment, cross-page bad fragment, missing file, and a fragment existing elsewhere but not locally. All four caught | 691 anchors, 435 fragments, 0 problems |
| Names-index checker | **v1 was broken** and reported 88 false positives; caught because it contradicted the already-validated link checker. v2 re-probed with three injected rows, all caught. No finding was ever reported from v1 | 88 rows, 0 problems |
| Source-line verifier | 70 `git show HEAD:` reads, each producing distinct content matching its label. `git show HEAD:` throughout, because the working tree is dirty | 70/70 correct |
| `bitflags` negative | Positive control on the same grep before trusting the negative, and run without a pipe so `$?` is grep's rather than `head`'s | confirmed absent |
| Tokeniser measurement | First pass classified on `class=` instead of `data-lang` and returned a visibly wrong 0/0/87, so it was re-read. The `JSON.parse` counter self-validates: 21 blocks do parse, proving the entity-decode step works | all figures exact |
| Snapshot freshness | `freeze-console --check` run before relying on any snapshot prose | ok |

**Retracted rather than reported.** An asserted twelfth finding, that two
`KNOWN_DRIFT` predicates were one-sided document-only checks, was refuted by
reading their derivations: both are two-sided conjunctions pairing a document
fact with a code fact. It was withdrawn before reporting rather than counted.

**Could not validate:** no headless browser was available to the falsifier, so
F1 to F4 rest on the static artifact chain (script tags, entry source, built
bundle bytes, import graph, zero `data-island` reads) rather than on executing
the page. I consider that airtight and it is flagged as static evidence.

---

# Accessibility and responsive

Run through the committed `scripts/cdp-audit.mjs` against a current `dist/`.

**40 of 40 page-width measurements clean.** Ten pages by four widths, comparing
`document.documentElement.scrollWidth` against `window.innerWidth`, with
`innerWidth` read back on every run so a clamped window cannot report a false
overflow.

| Width | Overflow | `main` width | Two-pane layout | Rail | Chips |
|---|---|---|---|---|---|
| 360 | none, 10/10 | 360 | `block` | hidden | shown |
| 768 | none, 10/10 | 768 | `block` | hidden | shown |
| 1440 | none, 10/10 | 1237 | **`grid`** | shown | hidden |
| 2560 | none, 10/10 | **1523** | `grid` | shown | hidden |

**180 focus stops walked with real `Input.dispatchKeyEvent` Tab presses, zero
without a visible indicator**, sampled 200ms after each press so the 120ms
opacity transition has settled. The skip link is the first stop on every page
walked. Never `element.focus()`, which gave two wrong readings in a row on this
same control in a previous audit.

**Semantics clean**: exactly one `<main>` and one `<h1>` per page, no skipped
heading levels, every `<nav>` labelled, no two visible navs sharing a label.

**Both C4b accessibility fixes confirmed in a browser rather than in markup**:
`console/all.html` and `console/names.html` carry **zero** `aria-current` marks,
and activating the skip link with a real Enter keypress moves
`document.activeElement` to `<main id="main">` on every page walked.

**Contrast**: all 20 gating pairs pass, weakest at 4.70. It audits token
definitions, not rendered pixels, and covers no non-text contrast.

## A hole in the instrument I committed in commit 1

`cdp-audit.mjs` checked the skip link by reading the `tabindex` attribute and the
first tab stop's href. That is the precondition, not proof: it never pressed the
link and never looked at where focus went. The instrument built to stop this
project trusting unverified claims was making one, in the same family as the
tautological gates but one level up, in the tool rather than the gate.

It now performs a real activation. **I injection-proved the new check**, which
the agent that wrote it did not: removing `tabindex="-1"` from `dist/console.html`
alone (probe confirmed landed: 0 occurrences on that page, still 1 on
`console/rpc.html`) makes the audit report `focus after Enter: <body>` for that
page and `<main id="main">` for the other two, failing with two problems. So the
activation check fires and is not redundant with the markup check.

Two further instrument bugs were fixed: a temp-profile cleanup race that threw
`ENOTEMPTY` after measurements had already succeeded, which in `--json` mode was
indistinguishable from a failed audit; and `--json` appending a human-readable
line that made stdout invalid JSON.

## Could not be tested

No real screen reader, no physical mobile hardware, no production environment.
Only three of ten pages were walked key by key, by the script's own design, since
the shared shell produces the same order elsewhere. Contrast covers token
definitions rather than rendered pixels.
