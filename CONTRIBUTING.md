# Contributing to NOVAI

## Clean-Room Policy

NOVAI is a clean-room implementation. This is a hard requirement, not a guideline.

**Do not** copy, translate, or adapt code from:
- Substrate, Tendermint, HotStuff reference implementations
- Diem, Cosmos SDK, Aptos, Sui
- Any other blockchain consensus implementation

If you have previously worked on any of these codebases, you may still contribute, but you must implement from first principles. If your PR contains code that resembles an existing implementation, you will be asked to rewrite it.

Papers and specifications are acceptable as concept-level inspiration. The implementation must be original.

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0. Any contribution intentionally submitted for inclusion in this project shall be under the terms and conditions of this license, without any additional terms or conditions.

## Dependencies

Before adding any new dependency to any `Cargo.toml`:

1. Verify the license is permissive (MIT, Apache-2.0, BSD, ISC, Zlib)
2. GPL and AGPL dependencies are forbidden — `cargo deny check licenses` enforces this
3. Open an issue or PR discussion before adding the dependency
4. Include the license verification in your PR description

## Code Standards

- All code must pass `cargo fmt`, `cargo clippy --all-targets`, and `cargo test --workspace`
- No `unsafe` code (enforced by `unsafe_code = "forbid"` in workspace Cargo.toml)
- All encoding formats must have golden vector tests
- All public functions in library crates need `# Errors` documentation if they return `Result`
- No floating point arithmetic in consensus or execution paths
- No nondeterministic iteration (e.g., `HashMap` iteration order)

## Consensus and Execution Changes

Changes to `crates/consensus/` or `crates/execution/` require extra scrutiny:

- Open an issue describing the change before submitting a PR
- Include a safety argument: why this change does not break consensus determinism
- If you change any encoding format, update the golden vector tests
- If a test fails in these crates, do not auto-fix — diagnose the root cause first

## Pull Requests

- Keep PRs focused. One logical change per PR.
- Write a clear description of what changed and why
- Reference any related issues
- All CI checks must pass before review

## Reporting Issues

Use GitHub Issues. Include:
- What you expected to happen
- What actually happened
- Steps to reproduce
- Relevant logs or error messages

For security vulnerabilities, see [SECURITY.md](SECURITY.md).
