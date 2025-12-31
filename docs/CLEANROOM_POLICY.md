# Clean-Room Policy (Binding)

This repository is developed under strict clean-room rules.

## Forbidden
- Copying, translating, or structurally adapting code from:
  Substrate, Tendermint, HotStuff implementations, Diem, Cosmos SDK, or similar
- Introducing GPL, LGPL, or AGPL licensed dependencies
- Non-deterministic logic in consensus or validity paths

## Allowed
- Reading academic papers and protocol specifications
- Implementing designs from first principles
- Using permissively licensed crates after license review

## Process
- All dependencies must be justified and license-checked
- `cargo-deny` must pass at all times
- Deviations require explicit written justification

Violations invalidate the clean-room guarantee.
