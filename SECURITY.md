# Security Policy

## Reporting Vulnerabilities

If you discover a security vulnerability in NOVAI, **do not open a public issue**.

Email: **NOVAInetwork@protonmail.com**

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if you have one)

You will receive an acknowledgment within 48 hours. We will work with you to understand the issue and coordinate disclosure.

## Scope

The following are in scope for security reports:

- Consensus safety violations (forks, double-commits, liveness failures)
- Transaction replay or nonce bypass
- Signature verification bypass
- State root divergence between honest nodes
- Denial of service against the consensus layer
- Memory safety issues (should not exist — `unsafe` is forbidden)
- Private key exposure through logging or error messages
- RPC endpoint vulnerabilities

## Out of Scope

- Issues in development tools (tx-generator, genesis-generator) that don't affect the node
- Performance issues that don't constitute denial of service
- Issues requiring physical access to the machine running the node

## Disclosure

We follow coordinated disclosure. We will:

1. Confirm the vulnerability and determine its impact
2. Develop and test a fix
3. Release the fix
4. Credit the reporter (unless they prefer to remain anonymous)

We ask that you do not publicly disclose the vulnerability until we have released a fix.

## Known Limitations

NOVAI is pre-mainnet software under active development. Known limitations:

- The validator set is fixed at genesis (no dynamic membership yet)
- There is no staking or slashing mechanism
- The NNPX privacy layer types are defined but logic is not active
- AI entity autonomous execution requires governance gate work that is not yet complete
- The faucet endpoint is only available in dev mode and has no rate limiting beyond the global RPC rate limit (100 req/s)
