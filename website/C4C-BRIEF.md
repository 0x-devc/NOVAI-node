# Gate C4c: the fifteen, and the assurance question

Written 2026-09-04, at the close of Gate C4b.

**Read this with `C4B-VERIFICATION.md`.** That file carries the evidence: what
was attacked, how each finding was verified, and which instruments were validated
before use. This file is the work list.

Nothing here is public. The console carries `noindex`, the site does not link to
it, and production returns an SPA catch-all for every path, so the console is not
served at all. This is debt, not an incident.

---

## Read this before the work list

**The gate did not converge, and that is the most important output.**

13 of 241, then 13 of 341, then 15 of 226. Flat to rising count, and the rate
rose to 6.6%. The prediction registered before run 3 was 4 to 7 falsified with 0
to 2 self-inflicted; the falsifying criterion was ten or more. Fifteen came back.

**Seven of the fifteen were caused by C4b itself.** So a third of the work of
each correctness gate is repairing the previous one. The instrument substantially
measures change volume.

**The eight that were not caused by C4b share one shape**, and it is the shape
this whole approach cannot see:

> Every claim sourced from the tree survived a third attack. What broke was the
> page describing behaviour the built artifact does not have.

70 source links, every enumeration, every fee, every payload arithmetic, the
whole canonical encoding table, and the C4b headline fix all held. The panels the
console showcases do not exist.

**So the first item of this gate is not a defect fix.** It is deciding whether
generate-then-audit continues, and what replaces or supplements it. See section
"The assurance question" at the end, and note it is genuinely open: I recommend a
direction there but the operator has not ruled.

---

## P0. The console publishes functionality that does not exist

**This is the most serious defect the console has carried and it must be closed
before any deploy, ahead of everything else in this file.**

Published in the present tense on two pages:

- `console/verify.html`: "This panel calls the live chain and re-derives a
  consensus property: it walks consecutive block headers and checks that each
  one's parent hash is the previous block's hash."
- `console/verify.html`: "This panel needs JavaScript. With it disabled the page
  still renders in full; only this check is unavailable."
- `console.html`: "Live height and cadence load here when JavaScript is
  available."
- `src/console/console-main.ts`, in a comment: "From gate C4 it also mounts the
  two React islands."

None of it is true. The built console entry is 29 bytes of stylesheet import,
zero bundles contain the string `data-island` while both container elements are
in the markup, and `VerifyPanel` is imported only by its own test and by the dev
specimen that `vite.config.ts` excludes from the build. `HANDOFF.md` schedules
the panels as Gate C5, unbuilt.

**Two honest resolutions, and the choice is the operator's:**

1. **Build C5 now**, so the sentences become true. This is the larger option and
   it needs the RPC endpoint resolver module that does not yet exist.
2. **Change the prose to match the artifact**, and say plainly that the live
   panels are not built yet. Smaller, immediately honest, and it keeps the page's
   credibility intact while C5 is scheduled properly.

**Do not pick option 2 by writing "coming soon".** State what the page does and
does not do, in the register the rest of the console uses.

**Then gate it**, because this is the class the whole page is about. Every
present-tense operational claim must be tied to an assertion against the BUILT
artifact. Concretely: a claim mentioning a panel requires the built bundle to
contain that panel's mount. This is the check whose absence let four false
claims survive two adversarial passes.

---

## P1. The six defects C4b introduced, in order of how badly they read

All six are mine. They come before the pre-existing ones because a gate that
introduces defects while fixing defects has to stop doing that first.

1. **A fabricated justification.** `generate-console-html.mjs` publishes that
   capability bits carry no citation "because they come from a bitflags
   declaration with no per-bit line to point at." There is no `bitflags` anywhere
   in `crates/` or `sdk/`, in any `.rs` or any `.toml`, and there ARE per-bit
   lines: `crates/ai_entities/src/lib.rs:170` reads
   `submit_reputation_updates: (byte & (1 << 5)) != 0`.

   Fix: cite the bits like everything else, since the lines exist. Delete the
   carve-out rather than reword it.

   This one is worth a rule, not just a fix. **A sentence explaining why
   something cannot be done is a claim about the source tree and must be
   verified like any other.** It was written inside the gate whose subject is
   exactly that, which is how easily this happens.

2. **A false universal wrapped around a true count.** "Every constant above is
   cited to the line that declares it: 10 citations across 8 declarations."
   `DEFAULT_REPUTATION_SCORE`, `MAX_CATALOG_OFFERINGS` and `STAKE_LOCK_PERIOD`
   are uncited. The count is gated and correct; the universal is not gated and is
   false.

   Fix: cite the remaining constants (they all have declarations), or state the
   coverage rather than a universal. Then gate the universal the way the count is
   gated, so the sentence cannot claim more than the page renders.

3. **The new lead sentence contradicts the aggregate page 18 times.** "the
   responses on this page were captured by running them" is true on
   `console.html` and false on `console/all.html`, where it sits above 18
   captions reading "Example response from the reference ... not a current
   reading".

   Fix: the sentence is page-scoped, so render it page-scoped. This is the same
   class as the "below" problem: prose written for one page, inherited by the
   aggregate.

4. **One of the five new cross-reference notes describes text that is not
   there.** It asserts 'The comments above say "below"' above a fence reading
   `// bitfield (see Capabilities)`, which has no "below". Four of five are
   correct.

   Fix: derive the note's wording from the phrase actually matched, rather than
   asserting one wording for all five.

5. **"Shared by every method in Signal methods" is now false**, on all three
   Signal methods, because C4b correctly stopped `getSignalsByHeight` sharing the
   range row. It carries 2 error rows; the other two carry 3.

   Fix: the note should describe what the console renders, not what the reference
   declares. If a category's rows are no longer identical across its methods, say
   so, ideally naming the scoping.

6. **The staleness warning in `scripts/tokenise.mjs` now points at nothing.**
   Every figure beside it is exact. C4b replaced the stale numbers and added the
   warning in the same commit, so a reader is told to distrust correct data.

   Fix: rewrite it as what it is, a note that these figures are measured and must
   be re-measured when the page changes.

---

## P2. The gate hole C4b introduced

**`sellersAreUncapped` reads a comment, not code.** In
`generate-console-data.mjs`, the predicate tests
`/Sellers are not capped/i` against the rustdoc preceding
`MAX_SLAS_PER_ENTITY`, while the comment beside it claims to measure the
constant's own declaration.

Ten of the eleven exceptions pair a document fact with a CODE fact. This one
pairs a document fact with another comment, so adding a seller cap without
touching the rustdoc would leave the exception holding and the console publishing
"Size for an unbounded result set rather than for eight."

The conclusion is nonetheless true and a code-side measurement is cheap:
`MAX_SLAS_PER_ENTITY` has exactly one enforcement site,
`crates/execution/src/lib.rs:10542`, in the SLA create handler, counting the
BUYER's own objects, and `handle_list_slas_by_seller` applies no cap at all.

Fix: measure the enforcement sites, not the prose about them. Then sweep the
other ten predicates for the same shape.

---

## P3. The four pre-existing defects that are not the panels

7. **`novai_getLatestBlock`'s null citation points at the wrong path.** It cites
   `rpc.rs:3606`, the `if height == 0` empty-chain guard; the record-missing path
   is `:3631`. Worse, the note's pruning-horizon clause cannot apply to a method
   that reads the tip. The other three null citations are correct, so this is
   boilerplate applied to the one method it does not fit.

   Note while fixing: `getBlockByHash` has TWO null paths, `:3592` (block not on
   disk) and `:3595` (hash not in the in-memory index), and only the first is
   cited. The second is the one that fires after a restart, which the console's
   own "an indexer cannot backfill" gap describes.

8. **The `submitTransaction` example is three inconsistent numbers.** The caption
   says "a real ~300-byte hex blob"; the field is the literal placeholder
   `01<224 hex chars>`, so it is neither real nor 300 bytes; 226 hex chars is 113
   bytes; and the console's own arithmetic (`TX_V1_OVERHEAD` 149 + `payload_len`
   83) gives 232 bytes, or 464 hex chars.

   Fix: either publish a real blob of the stated size or describe the placeholder
   as a placeholder and state the true length from the page's own arithmetic.
   Inherited from `docs/RPC_REFERENCE.md` and currently carried as no exception.

9. **The `getAiEntity` example response omits 7 of 20 always-serialised fields**,
   including `reputation_score` and `reputation_events_count`, which are the exact
   two the entities page tells the reader to check. `struct AiEntityJson` has zero
   `skip_serializing_if`, so the node cannot emit the published shape.

   Fix: this needs a real response. See the operator asks below.

10. **`StakeWithdraw`'s "Rejected unless" names one of three gates.** The handler
    also rejects on `InsufficientStakeBalance` and on
    `StakeWithdrawWouldUnderfundSlaCollateral` (`crates/execution/src/lib.rs:7782`),
    so an SLA seller meeting the published condition is still rejected.

    Fix: the description is quoted from a Rust doc comment but published in the
    console's own Description column. Either publish all three conditions or stop
    presenting a doc comment as a complete specification.

---

## The assurance question, which is the real deliverable

Three correctness gates have now each closed a batch and opened a smaller one.
The count is not falling. The honest reading is that **generate-then-audit is the
wrong primary assurance model for this page**, and the evidence is specific
rather than general:

- Everything DERIVED from the tree is now extremely solid. Three runs have failed
  to break a single source link, count, fee or arithmetic. That machinery works.
- Everything ASSERTED about the page's own behaviour is unprotected. Four false
  operational claims survived two adversarial passes of 241 and 341 claims,
  because auditing prose against `crates/` cannot reach a claim about `dist/`.

**Recommended direction, not yet ruled on.** Extend the gating discipline that
already works on numbers to claims:

1. **Every present-tense operational claim binds to an artifact assertion.** The
   `PROSE` object already centralises hand-written sentences. Tag the ones that
   assert runtime behaviour, and require each to name a check against the built
   output that must pass for it to render.
2. **Page-scoped prose must declare its scope.** Two of C4b's six defects (the
   lead sentence, and the "below" note) are one bug: a sentence true on the page
   it was written for and false on the aggregate. The renderer knows which page
   it is rendering; sentences that depend on that should have to say so.
3. **A claim explaining an absence is still a claim.** The fabricated bitflags
   justification would have been caught by treating "because X" as a assertion
   about the tree requiring the same verification as "X is 42".

The CI job already approved (see `HANDOFF.md` section 11) is the first item of
the next gate regardless, and it is complementary rather than an alternative: it
turns drift into a repo-time failure, but it would not have caught any of the
fifteen.

---

## Operator asks, carried forward

**Five are now closed.** Production serves an SPA catch-all for every path; the
published `blob/main` links are correct today because remote `main` and local
HEAD differ in no cited file; and on 2026-09-04 the operator measured the live
fleet and answered three more:

- **Validators: four, running.** "Validators | 4" is true today. Keep it hand-set
  and excluded from the generator, with the provenance stated at the site, because
  the repository's genesis files say 1, 5 and 5 and a generator reading the repo
  would publish a wrong number confidently. `NEEDS-OPERATOR.md` item 3b.
- **`--faucet-key` is not set.** The HTTP faucet route answers 503 on this fleet;
  the JSON-RPC method still answers via the dev-key fallback. So "there is no
  funding path" would be FALSE, and the console wording is an open decision with
  a proposed sentence awaiting a ruling. Item 2.
- **There is no genesis chain id**, because the fleet runs `--dev-keys` and reads
  no genesis file. The omission is now justified rather than pending, and the
  console's current reason ("the value that is live has not been confirmed") is
  stale and should be replaced. Item 3.

**None of these unblocks any of the fifteen.** They close operator questions and
one unverifiable; the fifteen are code and prose fixes that were never waiting on
server data. The one falsification that IS blocked on it is P3 item 9, below.

Still open:

1. **One live `entity_id` and the full `novai_getAiEntity` JSON for it.** This is
   the only one of the fifteen that server data can close: it settles whether the
   wire response really carries all 20 always-serialised fields, which is P3
   item 9. Operator is looking.
2. **Rate-limit and oversize behaviour from a host you control**: 130 sequential
   POSTs for the 429, and a 600 KB body for the 413, with raw bodies so the
   published strings can be compared byte for byte. The public endpoint was
   deliberately not stressed from here. Closes unverifiable 4, not a
   falsification.
3. **The intended npm package name.** `novai-sdk-ts` is 404; the SDK page says
   "build it" without naming a package.
