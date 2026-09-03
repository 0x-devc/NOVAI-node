# Operator actions

Running list of server-side, docs, or operator-only actions the website depends
on. Nothing here is required for the site to function; each item unlocks a
labeled degraded state, or records a defect I cannot fix from `website/`.
No secrets, no hosts beyond the public domains, no paths.

Two groups: server and hosting actions, then documentation fixes. The
documentation fixes each correspond to a live exception in the console's drift
gate, and every one of those exceptions is designed to be deleted once the doc
is fixed. The gate fails if an exception stops applying, so this list can only
shrink.

---

# Server and hosting

## 1. RPC CORS: RESOLVED, no action needed

Superseded 2026-08-26. The RPC at https://rpc.novai.network now answers with
`Access-Control-Allow-Origin: *`. I verified this directly:

    OPTIONS preflight, Origin https://novai.network  ->  204, ACAO: *
    POST                Origin https://novai.network  ->  200, ACAO: *
    OPTIONS preflight, a deliberately invalid origin  ->  204, ACAO: *

The third check matters: it is a true wildcard, not an origin echo, so no
per-origin allowlist is involved and nothing needs adding.

Caveat, and the reason the fallback stays built: this is a live observation, not
a configuration fact. It can go false without warning if the proxy config
changes. The verify panel keeps its terminal mode and the network panel keeps
its snapshot state, both tested, so a future tightening degrades to a labeled
state instead of a broken page.

## 2. Faucet: is `--faucet-key` set on the live testnet?

I need a yes or no. It decides what the console publishes about funding.

The public HTTP route `GET /faucet/<address>` is gated solely on `--faucet-key`.
It is NOT gated on dev mode, contrary to the RPC reference (see item 7). So:

- If the flag IS set: the route is live in production, dispensing 100,000 per
  request with a 24 hour per-IP cooldown, and I document the funding path as
  information. No faucet UI either way.
- If the flag is NOT set: the route answers 503, and the console says plainly
  that there is no public funding path yet. A developer who discovers that after
  writing code is worse off than one told up front.

Note the asymmetry: the JSON-RPC `novai_faucet` method is enabled by
`--faucet-key` OR `--dev-keys`, while the HTTP route requires `--faucet-key`
specifically. A plain dev-keys node therefore has the method but not the route.

## 3. Chain id: which value is the live one?

I am omitting chain id from the console until this is answered, because I cannot
determine it from the repo and a wrong answer here is the kind of thing a
developer hardcodes.

Two unrelated things carry the name:

- A protocol constant equal to 1, used only in channel-state signing. I am NOT
  publishing this: it would be read as the network identifier and it is not.
- A human readable genesis string, which takes three different values across the
  devnet, testnet and mainnet configs in the repo.

The complication: in `--dev-keys` mode the node never reads a genesis file at
all, so none of the three repo values is necessarily what is running.

## 4. Genesis hash: not retrievable, confirming omission

No action required, recorded so the decision is not revisited.

The genesis hash is computed at runtime from genesis state, so it is not a repo
constant. I tried to derive it from the chain instead and it is not reachable:
block 0 and block 1 both answer `null`. That is pruning, not absence. The node
retains `PRUNE_RETAIN_BLOCKS` (50,000) blocks, so genesis has long since fallen
out of the served window.

The console therefore omits the genesis hash and instead states, in known gaps,
that block 0 is not retrievable. That is the more useful fact anyway.

## 5. Refresh the chain snapshot before any deploy

One command answers whether you are clear to push:

    npm run predeploy

It refreshes the snapshot, verifies it, runs the test suite, then builds. Note
that it WRITES `src/data/chain-snapshot.json`, so commit that file as part of
deploying. The individual steps are still available:

    npm run snapshot          # refresh only
    npm run snapshot:check    # verify only, exits 1 if the snapshot is unusable

Why `predeploy` refreshes rather than only checking: the retention window is
about 3 hours of wall clock (see below), so a check-only command would fail on
essentially every invocation and would stop being read. Refreshing first makes
a green run mean something.

`snapshot:check` reads the retention window from `PRUNE_RETAIN_BLOCKS` in the
consensus crate rather than hand-typing it, fetches the live tip, and fails if
the committed snapshot has fallen outside the window or is ahead of the tip
(which would mean a chain reset).

Be aware of what this can and cannot be. The window is a configuration fact in
blocks, not in time: `PRUNE_RETAIN_BLOCKS` is 50,000. The wall-clock equivalent
is whatever current cadence makes it, and cadence is NOT stable. It measured
about 1.1 blocks/sec when this work started and about 4.8 blocks/sec a few days
later, which moves the window from roughly half a day down to under three hours.
Do not rely on a remembered figure; if you need the current number, measure it.

A committed snapshot is therefore usable for a matter of hours after capture.
This is a run-it-immediately-before-deploy tool, not a routine gate, and it is
deliberately NOT wired into `prebuild` because builds are hermetic and network
free.

The durable fix is not snapshot hygiene. The panels no longer treat any snapshot
value as retrievable: the snapshot is a labeled historical display only, and the
height input is never seeded from it.

## 6. Console URL: `/console` versus `/console.html`

The build emits `dist/console.html`, which a plain static host serves at
`/console.html`. Two things need checking on the host, and I do not want to
guess at either:

1. Does the host rewrite `/console` to `/console.html`?
2. Is there an SPA catch-all that rewrites unknown paths to `index.html`? If so
   it will swallow `/console` entirely.

**Question 2 is answered: yes.** Measured 2026-09-03, and anyone can repeat it:

    for p in / /console /console.html /console/rpc.html \
             /console/zzz-does-not-exist.html; do
      curl -s -o /tmp/b "https://novai.network$p"
      echo "$p $(wc -c < /tmp/b) $(shasum -a 256 /tmp/b | cut -c1-12)"
    done

Every one of those paths returns HTTP 200 with the same 1,295-byte body, sha
`8813482f9260`, including a URL that cannot exist. That is an SPA catch-all.

Two consequences follow, and the second is the one that matters:

- Question 1 is moot until a deploy happens. `/console.html` and
  `/console/rpc.html` are swallowed exactly like `/console` is, so no rewrite
  rule is being exercised either way.
- **Production is not serving the console at all**, and has not been. Whatever
  is decided about the URL shape, the catch-all has to be narrowed to let real
  files through before any of it takes effect.

An earlier note in the C4b planning recorded 1,294 bytes and a different sha.
Today's measurement is the one above. I have not tried to reconcile the two,
because I cannot source what the earlier one measured and inventing a reason
would be worse than recording that they differ.

## 7. Explorer link (for you to add, I do not touch that deployment)

Suggested markup for the explorer to link the developer console:

    <a href="https://novai.network/console">Developer console</a>

Adjust the path per item 6 once the host behaviour is known.

---

# Documentation fixes

These are defects in `docs/`, which is outside the website-only scope I work
under, so I cannot fix them. Each is carried as an explicit, justified exception
in the console's drift gate, printed on every build. Fix the doc, then delete
the exception: the gate will tell you to, because it fails when an exception no
longer applies.

## 8. `-32014 NonceTooHigh` is emitted but undocumented

This one is a real client-breaking bug, not cosmetic.

The code is emitted at `crates/node/src/rpc.rs:2060`. It appears zero times in
`docs/RPC_REFERENCE.md`, whose error table lists twelve codes and stops at
`-32013`. The inline comment at `rpc.rs:2043` already documents it internally.

Why it breaks clients: the doc's guidance for `-32010` (nonce too low) is to
resync, because the transaction is dead. The correct handling for `-32014`
(nonce too high) is the opposite: retry unchanged, because the transaction is
merely early and succeeds once the sender's earlier nonces commit. A client
built from the published table will do the wrong thing on every nonce-ahead
submission.

## 9. `GET /faucet` gating is documented backwards

`docs/RPC_REFERENCE.md:18` says the route is "available only when the node is
launched in Dev-mode". Both halves are wrong:

- `handle_public_faucet` takes no dev-mode parameter at all. Its signature
  carries `faucet_key` only, and it returns 503 when that is absent.
- It therefore runs in production whenever `--faucet-key` is passed, and does
  NOT run on a plain dev-keys devnet.

An operator reading the current text could conclude the route is self-limiting
when it is not. The doc also omits the 100,000 dispense amount, the 24 hour
per-IP cooldown, that the cooldown persists across restarts, and the
trusted-proxy allowlist.

## 10. `novai_faucet` disabled-path error code is wrong

`docs/RPC_REFERENCE.md:1583` lists `-32602` for "node not in dev-mode". The
source returns `-32000` (`crates/node/src/rpc.rs:3005`). The key-resolution
check also runs before the params are parsed, so on a node without a faucet even
a malformed address returns `-32000`, never `-32602`.

## 11. Every RPC example assumes a loopback endpoint

`docs/RPC_REFERENCE.md` states that all examples assume `URL=http://localhost:3030`,
and the transport section gives a loopback default. The 29 curl examples
themselves are fine: every one of them uses `$URL`, so they are portable as
written and I emit them unchanged.

The problem is only that one assumed value, and it is expensive. A developer
reading the repo has no way to learn that a public endpoint exists, so the
fastest apparent path to a first call is to build the node and run a local
devnet. Publishing the public endpoint in the transport section is a one line
fix and probably the highest-value edit in the whole document.

## 12. The retention window is missing from the Observed gaps table

`docs/RPC_REFERENCE.md` has an Observed gaps table listing six gaps. It does not
mention that a node serves only the last `PRUNE_RETAIN_BLOCKS` (50,000) blocks
and prunes everything older, which at current cadence is roughly 3.5 hours of
history.

Anyone building an indexer needs this before they start, not after. The console
publishes it as a generated configuration fact regardless, but the doc should
carry it too.

## 13. `novai_faucet` is documented as dev-keys only, but accepts `--faucet-key`

`docs/RPC_REFERENCE.md:1555` says the method is "available **only** when the
node was launched with `--dev-keys --allow-insecure-dev-keys`". `handle_faucet`
resolves its key in three steps: use `--faucet-key` if one was loaded, else fall
back to the deterministic dev key when `--dev-keys` is set, else refuse. So the
method runs on a production node whenever a faucet key is present.

This is item 9 again on a different surface. Item 9 is the HTTP route, this is
the JSON-RPC method, and both read as self-limiting to devnets when neither is.
Fixing one without the other leaves the document half wrong.

The console carries this as drift exception `faucet-rpc-gating-incomplete`,
printed on every generator run. Fixing the sentence forces the exception to be
deleted, because the gate fails when a listed exception stops applying.

## 14. `CONTRIBUTING.md` omits the gate test convention

31 of the 121 test files under `crates/*/tests/` are named `gate_*`, and the
convention carries real meaning about how a change is expected to be proven.
`CONTRIBUTING.md` is 62 lines and never mentions it. Its "Consensus and
Execution Changes" section comes closest, requiring a safety argument and
forbidding auto-fixes, but it does not tell a contributor that gate-named tests
exist or what makes one.

A contributor meets this convention within minutes of opening the test
directory and has nothing to read about it. The console's contributing section
will describe it, but the console describing a repo convention that the repo
itself does not document is the wrong way round.

---

## 15. `novai_getNonce` is documented as interchangeable with `getBalance`

`docs/RPC_REFERENCE.md` describes `novai_getNonce` as "Cheaper than
`getBalance` if you don't need the balance." The two answer from different
sources and are not substitutes.

- `handle_get_nonce` returns `nonce_provider.expected_nonce(&address)`
  (`crates/node/src/rpc.rs:2105`), the in-memory mempool admission cursor.
- `handle_get_balance` returns `account.nonce` from
  `read_account_or_default` (`crates/node/src/rpc.rs:2719-2727`), the
  committed state row.

The cursor advances on every committed transaction regardless of whether
execution succeeded (`crates/node/src/main.rs:250-266`, with the reasoning in
its own comment). The account nonce advances only on success: the equality
check at `crates/execution/src/lib.rs:6922` returns before any write. So after
one committed-but-failed transaction from a sender the two diverge, and the
cursor stays ahead until the node restarts and reseeds from state
(`crates/node/src/main.rs:136-149`).

Consequence for a client: build plain-account transactions from
`getBalance.nonce`, and use `getNonce` only to predict whether the mempool will
admit. A client following the doc's wording signs a nonce execution will
reject. Registered AI entities are unaffected, because their path compares with
`>=` rather than equality and self-heals.

This is a documentation defect, not a chain change, and nothing here proposes
one. It is carried as the `getnonce-documented-as-interchangeable` entry in
`KNOWN_DRIFT` and is printed on every generator run. The console publishes the
distinction in its known-gaps section. Fixing the doc's wording retires the
exception, and the gate will then fail until it is deleted.

## 16. `novai_listVkRegistrations` inherits an error clause naming `id`

`docs/RPC_REFERENCE.md` aliases this method's Errors block onto
`novai_getVkRegistration`, and the inherited `-32602` clause reads
`` `id` isn't 32 bytes ``. This method declares no `id`. Its only parameter is
`entity_id`, validated at `crates/node/src/rpc.rs:2371` as
`parse_hex32(&params.entity_id, "entity_id")`, so the rejection a caller
actually sees names `entity_id`.

The method block used to contradict itself three ways: the params table said
`entity_id`, the error clause said `id`, and the curl passed `entity_id`.

Fixing the alias line so the clause names this method's own field retires the
`vk-list-error-clause-names-foreign-field` exception, and the gate will then
fail until it is deleted.

---

## 17. `novai_getNonce` inherits a `-32002` it cannot emit

The reference aliases this method's Errors block onto `novai_getBalance`, which
brings a `-32002 DB read failure` row with it. `handle_get_nonce`
(`crates/node/src/rpc.rs:2082`) takes `(request, nonce_provider)` and its
dispatch arm passes no database. Its whole body is a hex parse plus
`nonce_provider.expected_nonce`, and `-32002` appears nowhere in it, while
`handle_get_balance` does read state and does emit it.

A client writes a storage-retry branch that is dead code, and mis-attributes the
failures it does see. Giving the method its own two-row Errors block retires the
`getnonce-inherits-unreachable-db-error` exception.

---

## 18. `novai_getBlockByHeight`'s null answer is called unreachable

The result block reads "or `null` if no such height (this should be unreachable
given the validation)". The handler returns a top-level null whenever the block
is not on disk, and a node retains `PRUNE_RETAIN_BLOCKS = 50,000` blocks, so
every height below the horizon answers null. That is normal operation, and the
console's own known-gaps section already says so.

A client written to the parenthetical does not null-check and breaks the first
time it reads history. Deleting the parenthetical retires the
`blockbyheight-null-called-unreachable` exception.

---

## 19. `-32600` is documented with the wrong trigger

The error table says `-32600` answers a "malformed JSON-RPC envelope (missing
`jsonrpc`/`method`)". Every field of `RpcRequest`
(`crates/node/src/rpc.rs:93-98`) is required, with no `Option` and no serde
default, so a missing field fails deserialization and answers `-32700`.
`-32600` is reachable only from `crates/node/src/rpc.rs:1348`, when `jsonrpc` is
present and is not `"2.0"`.

Verified live against the public endpoint: a request with no `method` answers
`-32700`, a request with no `jsonrpc` answers `-32700`, and
`"jsonrpc":"1.0"` answers `-32600`.

A client matching `-32600` to detect a malformed envelope never sees it, and
gets `-32700`, which the same table attributes to invalid JSON. Correcting the
trigger cell retires the `invalid-request-trigger-is-wrong` exception.

## 20. Console URL shape after the page split, and `/console/all.html` indexing

Two things to decide, neither urgent, both recorded now so they are not
discovered later.

**URLs.** The console is now nine pages plus an index of names. The landing page
stays at `/console.html` and the rest are `/console/rpc.html`,
`/console/errors.html`, and so on. That shape needs no rewrite rules and works
on plain static hosting, which is why I chose it while item 6 is still open. If
you would rather have `/console/` and `/console/rpc`, that is a hosting decision
and it changes every cross-page link the generator emits, so it should be made
before the console is announced rather than after.

**Indexing.** `/console/all.html` is a full duplicate of all eight pages, and
`/console/names.html` is an index of every name on them. That is fine today
because the whole console carries `noindex`, and it becomes a duplicate-content
problem the moment that comes off.

The decision, made now: **`/console/all.html` keeps `noindex, follow`
permanently, including after the rest of the console is indexed.**

Not a canonical link, and the reason is technical rather than preference.
`rel="canonical"` expresses a near-duplicate relationship between one page and
one other page. `all.html` is an eight-to-one aggregate, so there is no single
URL it could name, and pointing it at any one of the eight would be a false
signal about the other seven. `noindex, follow` is the mechanism for exactly
this case: the aggregate is not indexed, its links are still followed, and every
heading on it already links to the canonical page anchor, so nothing is
stranded. It also keeps the page reachable, which matters because it is the
single best URL to hand an agent, and the settled ruling is crawlable but not
indexed rather than blocked.

`names.html` is not a duplicate of anything and can be indexed normally.

---

## 21. `novai_getLatestBlock` is documented as answering only the global errors

`docs/RPC_REFERENCE.md:101` reads "**Errors**: only the global ones (`-32600`
malformed envelope, `-32601` unknown method)." The handler answers `-32002` on
two separate paths: `crates/node/src/rpc.rs:3611` when the block fails to load,
and `:3618` when it fails to hash.

This matters more than its size suggests. It is the method every integration
calls first, and it is the only one of the three block methods claiming
immunity: `novai_getBlockByHeight` and `novai_getBlockByHash` both document
`-32002`. A client written to the documented set meets an undocumented code on
the one call it is most likely to use as a health check.

Carried as the `latestblock-claims-only-global-errors` exception, printed on
every generator run, with the correction published under that method's error
table. Fixing the sentence retires the exception, and the gate will then fail
until it is deleted.

## 22. The `listSlasBySeller` cap sentence is false

`docs/RPC_REFERENCE.md:1148` says the method is "Bounded internally by the
per-buyer cap (= 8 in v1)". The constant's own rustdoc at
`crates/ai_entities/src/memory.rs:158-163` says the opposite in as many words:
the cap is per BUYER, the memory-object owner, and *"Sellers are not capped in
v1: they appear in arbitrarily many SLAs but never own the underlying memory
object."*

The consequence is data loss rather than an error the caller can see. A client
that sizes a fixed buffer on a documented guarantee of eight silently truncates
a seller's result set.

Worth recording how this was found, because it bears on how much weight an
agent's report carries. A Phase 1 agent examined this exact sentence, read the
rustdoc on the RPC handler rather than the one on the constant, and cleared it
as correct. The falsifier read the constant's own declaration and got the
opposite answer. The measurement now reads the constant, so the exception
retires itself when the doc is fixed.

Carried as the `sla-seller-cap-does-not-exist` exception, with the false
sentence struck at its own site and the truth published beneath it.

## 23. The deployed site predates the whole website workstream

Measured 2026-09-03. `https://novai.network/` returns a `<title>` containing a
U+2014 em dash:

    NOVAInetwork <em dash> AI-Integrated Layer 1 Blockchain

The repository's `index.html` has used an ASCII hyphen since the Gate 2.5 dash
gate landed, and the gate would now refuse that character. So what is deployed is
older than the redesign work, not one push behind it.

No action is proposed here and nothing about it is diagnosable from this side.
It is recorded because it changes how any statement about "the live site" should
be read: the deployed artifact is not what this repository builds, so a claim
verified against production is not a claim about this code.

## 24. Publishing `novai-sdk` to crates.io, which needs the path dependencies versioned first

The console's SDK section reports that the Rust SDK cannot be installed from a
registry, and that report is correct and should not be softened. This item is the
fix for the underlying product gap rather than for the reporting of it.

`sdk/novai-sdk/Cargo.toml` depends on four workspace crates by path, which the
generator reads and publishes:

| Crate | Path |
|---|---|
| `novai-types` | `crates/types` |
| `novai-crypto` | `crates/crypto` |
| `novai-codec` | `crates/codec` |
| `novai-ai-entities` | `crates/ai_entities` |

Cargo refuses to publish a crate carrying a bare `path` dependency, because a
consumer downloading from the registry has no such path. So publishing the SDK is
not one action, it is five, and the four come first:

1. Give each of the four crates a version and publish it to crates.io, in
   dependency order, so each one's own dependencies already resolve from the
   registry when it goes up.
2. Rewrite the SDK's four dependencies in the dual form
   `{ version = "x.y.z", path = "../../crates/..." }`. Cargo uses the path for a
   workspace build and the version for a published one, so local development is
   unaffected.
3. Publish `novai-sdk`.

Two things to check before starting, neither of which I can check from here:
whether those five names are available on crates.io, and whether any of the four
crates pulls in a further workspace crate by path, which would extend the list.

The console needs no change when this lands. It reads the manifest, so the
`consumableFromRegistry: false` it publishes today flips on its own, and the
sentence about the Rust SDK stops being true the moment the dependencies stop
being paths.

---

FYI, no action needed: the `rpc.novai.network` TLS certificate is valid until
**Oct 30 2026**, measured on 2026-08-30, and certbot last renewed it on Aug 1.

This entry previously said the certificate expired on 2026-08-30 and carried an
appended note reading "that is tomorrow". Both were wrong, and the second was
the worse kind of wrong: an inference from a stale line in this document,
written as though it were an observation. The measurement, which anyone can
repeat:

```
echo | openssl s_client -servername rpc.novai.network -connect rpc.novai.network:443 2>/dev/null \
  | openssl x509 -noout -dates
```

It matters because every runnable example on the console posts to
`https://rpc.novai.network`. An expired certificate does not degrade the page,
it makes the whole first-call section fail in the visitor's terminal.
