
Add mempool dependency to crates/node

Create a minimal
type (placeholder) in crates/types

Add a basic submit-tx entry point (CLI)
•
Insert tx into mempool and log tx id

Add a drain-mempool debug command to prove ordering

Add at least 1 integration/unit test for the wiring

Ensure
fmt / clippy / test / deny pass
[dev-dependencies]
proptest = "1.4"