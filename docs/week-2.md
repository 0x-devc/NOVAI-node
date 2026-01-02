•
•
•
Add mempool dependency to crates/node
Create a minimal Tx type (placeholder) in crates/types or crates/node
Add a basic "submit tx" entry point (CLI flag or simple stdin handler)
Insert tx into mempool and log tx id
Add a simple "drain mempool" debug command to prove ordering
Add at least 1 integration/unit test for the wiring
Ensure fmt / clippy / test / deny pass