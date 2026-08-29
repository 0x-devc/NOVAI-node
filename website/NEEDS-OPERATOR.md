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

Until this is confirmed, every internal link to the console sits behind a single
constant in the source, so switching between the two forms is a one line edit
once you tell me what the host actually serves.

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

---

FYI, no action needed unless renewal fails: the rpc.novai.network TLS
certificate expires 2026-08-30 with certbot auto-renewal configured. That is
close enough now to be worth a glance.

**Update 2026-08-29: that is tomorrow.** Worth confirming renewal actually
fired rather than assuming it did. Every runnable example on the console posts
to `https://rpc.novai.network`, so an expired certificate does not degrade the
page, it makes the whole first-call section fail in the visitor's terminal.
