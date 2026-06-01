# NOVAI Explorer

A block explorer for NOVAI: latest blocks, block / tx / account / AI-entity detail pages, and a real-time stats dashboard. React + Vite + Tailwind, talks to a local NOVAI node via JSON-RPC.

```
explorer/
├── package.json
├── vite.config.ts          (proxies /rpc → localhost:3030)
├── tsconfig.json
├── tailwind.config.js
├── postcss.config.js
├── index.html
└── src/
    ├── main.tsx            (router + entrypoint)
    ├── App.tsx             (header + outlet + footer)
    ├── index.css           (Tailwind layers + .card / .hex / .stat tokens)
    ├── lib/
    │   ├── rpc.ts          (typed wrappers for every RPC method)
    │   ├── format.ts       (hex shortening, u128 formatting, type-name maps)
    │   └── search.ts       (smart-route block height / hash / txid / address)
    ├── components/
    │   ├── Header.tsx
    │   ├── HashLink.tsx
    │   ├── Spinner.tsx
    │   ├── ErrorState.tsx
    │   └── EmptyState.tsx
    └── pages/
        ├── Blocks.tsx      (latest blocks, polled every 2s)
        ├── BlockDetail.tsx (by height OR hash)
        ├── TxDetail.tsx
        ├── Account.tsx     (balance, nonce, "is also an entity?" hint)
        ├── Entity.tsx      (entity record + memory objects + recent signals)
        ├── Stats.tsx       (height, blocks/sec, recent tx count, validators)
        └── NotFound.tsx
```

---

## Run it locally

**1. Start a NOVAI devnet** (in a separate terminal, from the repo root):

```bash
./scripts/devnet.sh
```

This launches four validators on `localhost:9000–9003` with JSON-RPC on `localhost:3030`. See [`docs/tutorials/FIRST_AI_ENTITY.md`](../docs/tutorials/FIRST_AI_ENTITY.md) for what the chain expects.

**2. Install + start the explorer**:

```bash
cd explorer
npm install
npm run dev
```

Open <http://localhost:5173>. The dev server proxies `/rpc` → `http://localhost:3030`, so the browser doesn't need any CORS configuration. Override the target by setting `NOVAI_RPC_URL`:

```bash
NOVAI_RPC_URL=http://my-node:3030 npm run dev
```

**3. (Optional) Production build** to verify the bundle compiles:

```bash
npm run build       # tsc + vite build → dist/
npm run preview     # serve dist/ on :4173 for spot-checking
```

---

## What's covered

The explorer reads from these JSON-RPC methods (all 8 read endpoints currently exposed):

| Page | RPC calls |
|---|---|
| Latest blocks (real-time) | `novai_getLatestBlock`, `novai_getBlockByHeight` (×N for the window) |
| Block detail | `novai_getBlockByHeight` or `novai_getBlockByHash` |
| Tx detail | `novai_getTransaction` |
| Account | `novai_getBalance` + `novai_getAiEntity` (probed in parallel) |
| AI entity | `novai_getAiEntity`, `novai_getMemoryObjects`, `novai_getSignalsByIssuer` |
| Stats | `novai_getLatestBlock` + windowed `novai_getBlockByHeight` for tx count |
| Search | probes `getBlockByHash` → `getTransaction` → falls through to account |

Submission methods (`novai_submitTransaction`, `novai_faucet`) aren't used — the explorer is read-only.

For the complete set of methods + request/response shapes, see [`docs/RPC_REFERENCE.md`](../docs/RPC_REFERENCE.md).

---

## Honest limitations

- **No transaction history per address.** The chain doesn't yet index txs by sender. The Account page shows a "tx history not yet indexed" panel until that lands.
- **Validator count is hardcoded "4 (devnet)".** No validator-set RPC exists yet; once it does the Stats page will probe it.
- **Total transactions is windowed.** Showing "txs in the last 100 blocks" rather than "txs ever" — total-since-genesis would require walking history block-by-block, which the explorer deliberately avoids on every page load.
- **Block detail page doesn't list the txs in that block.** The node exposes block headers via `getBlockByHeight` / `getBlockByHash` but not the per-block tx list. Look up individual txs via the search bar instead.

---

## Deployment notes (when you ship a public testnet)

1. Build once: `npm run build` → `dist/`.
2. Serve `dist/` from any static host (nginx, Caddy, Cloudflare Pages, etc.).
3. The static host **must proxy `/rpc/*`** to the node so the browser never makes cross-origin calls. Sample nginx:

   ```nginx
   location /rpc {
     proxy_pass http://novai-node:3030/;
     proxy_set_header Host $host;
   }
   ```

4. SPA fallback: any unknown path should return `index.html` (otherwise reloading a `/blocks/123` URL 404s).

   ```nginx
   location / {
     try_files $uri /index.html;
   }
   ```

That's it — no API server in front, no database, no backend. The explorer is purely a thin client over the node's JSON-RPC.
