# Operator actions

Running list of server-side or operator-only actions the website depends on.
Nothing here is required for the site to function; each item unlocks a labeled
degraded state. No secrets, no hosts beyond the public domains, no paths.

## 1. RPC CORS origins (unlocks live data on novai.network and the verify panel)

The RPC at https://rpc.novai.network currently allows the explorer origin
only. To let the public site read the chain from a visitor's browser, add
these origins to the Access-Control-Allow-Origin policy in the RPC nginx
config:

    https://novai.network
    http://localhost:8080

The localhost entry is optional: local development already reaches the RPC
through the dev-server proxy, so it only matters for testing a production
build locally with live data.

Until this change ships, the site shows the build-time snapshot (labeled as
such) and the verify panel runs in terminal mode: it shows the exact curl
commands for visitors to run themselves, plus a sample exchange captured at
build time.

FYI, no action needed: the rpc.novai.network TLS certificate expires
2026-08-30 with certbot auto-renewal configured.
