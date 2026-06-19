"""compute-oracle agent library.

Flat module layout, imported as ``lib.<module>`` from the top-level
``oracle.py`` and ``bootstrap.py`` scripts:

- ``config``: environment to a frozen ``ComputeOracleConfig`` dataclass.
- ``log``: structured logging setup matching the monitoring format.
- ``metrics``: thread-safe Prometheus text registry plus an HTTP server.
- ``gpu_source``: public GPU pricing fetch and parse, with an injectable
  HTTP opener and retry-friendly error classes.
- ``signal``: canonical observation encoding plus OracleAnchor and
  ReputationUpdate byte construction through the novai_sdk builders.
- ``chain``: the single funnel for chain interaction, with a DRY_RUN path
  that constructs and signs the transaction locally and never touches the
  RPC client.
"""
