"""AI entity capabilities bitmask.

The wire format is a single byte with one bit per capability. The bit
assignment matches ``crates/ai_entities/src/lib.rs`` exactly; reordering or
inserting new flags is a hard fork on the entity registration payload.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Capabilities:
    """8-bit capability flag set for an AI entity.

    Each flag occupies one bit:

    * bit 0: ``read_public_chain`` - read blocks, txs, accounts.
    * bit 1: ``read_memory_objects`` - read L1 memory object state.
    * bit 2: ``emit_proposals`` - emit proposal objects (and signals).
    * bit 3: ``request_execution`` - request Tier 1/2 execution via gates.
    * bit 4: ``read_nnpx_derived`` - read NNPX privacy derived views.
    * bit 5: ``submit_reputation_updates`` - oracle entities only.
    * bit 6: ``post_oracle_anchors`` - Week 35 OracleAnchor signal type.
    * bit 7: reserved.
    """

    read_public_chain: bool = False
    read_memory_objects: bool = False
    emit_proposals: bool = False
    request_execution: bool = False
    read_nnpx_derived: bool = False
    submit_reputation_updates: bool = False
    post_oracle_anchors: bool = False

    def to_byte(self) -> int:
        """Encode to the canonical 8-bit flag byte."""
        flags = 0
        if self.read_public_chain:
            flags |= 1 << 0
        if self.read_memory_objects:
            flags |= 1 << 1
        if self.emit_proposals:
            flags |= 1 << 2
        if self.request_execution:
            flags |= 1 << 3
        if self.read_nnpx_derived:
            flags |= 1 << 4
        if self.submit_reputation_updates:
            flags |= 1 << 5
        if self.post_oracle_anchors:
            flags |= 1 << 6
        return flags

    @classmethod
    def from_byte(cls, byte: int) -> Capabilities:
        """Decode from the canonical 8-bit flag byte."""
        if not 0 <= byte <= 0xFF:
            raise ValueError(f"capabilities byte must be in [0, 255], got {byte}")
        return cls(
            read_public_chain=bool(byte & (1 << 0)),
            read_memory_objects=bool(byte & (1 << 1)),
            emit_proposals=bool(byte & (1 << 2)),
            request_execution=bool(byte & (1 << 3)),
            read_nnpx_derived=bool(byte & (1 << 4)),
            submit_reputation_updates=bool(byte & (1 << 5)),
            post_oracle_anchors=bool(byte & (1 << 6)),
        )

    def __or__(self, other: Capabilities) -> Capabilities:
        """Union two capability sets (bit-OR semantics)."""
        return Capabilities.from_byte(self.to_byte() | other.to_byte())

    @classmethod
    def read_only(cls) -> Capabilities:
        """Minimal read-only set: read chain + memory, nothing else."""
        return cls(read_public_chain=True, read_memory_objects=True)

    @classmethod
    def advisory(cls) -> Capabilities:
        """Advisory entity: can propose but not request execution."""
        return cls(read_public_chain=True, read_memory_objects=True, emit_proposals=True)

    @classmethod
    def gated(cls) -> Capabilities:
        """Gated entity: can request execution through approval gates."""
        return cls(
            read_public_chain=True,
            read_memory_objects=True,
            emit_proposals=True,
            request_execution=True,
        )

    @classmethod
    def oracle(cls) -> Capabilities:
        """Oracle entity: read access plus the Week 35 anchor-posting capability."""
        return cls(
            read_public_chain=True,
            read_memory_objects=True,
            emit_proposals=True,
            post_oracle_anchors=True,
        )
