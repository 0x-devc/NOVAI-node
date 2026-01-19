// SPDX-License-Identifier: MIT OR Apache-2.0
//! Canonical encoding for approval gate types.
//!
//! PURPOSE: Provide deterministic binary encoding/decoding for ApprovalGate
//! to support storage, network transmission, and golden vector testing.
//!
//! ENCODING FORMAT (big-endian integers, variable length):
//!
//! | Offset    | Size      | Field              | Notes                      |
//! |-----------|-----------|--------------------|-----------------------------|
//! | 0         | 1         | version            | 0x01 for V1                |
//! | 1         | 32        | gate_id            | [u8; 32]                   |
//! | 33        | 1         | gate_type          | GateType discriminant      |
//! | 34        | 4         | approvers_count    | u32 BE                     |
//! | 38        | 32*N      | required_approvers | N addresses, 32 bytes each |
//! | 38+32N    | 4         | threshold          | u32 BE                     |
//! | 38+32N+4  | 8         | timelock_blocks    | u64 BE                     |
//! | 38+32N+12 | 8         | expiry_blocks      | u64 BE                     |
//! | 38+32N+20 | 1         | flags              | bit 0=veto, bit 1=freeze   |
//!
//! MINIMUM SIZE (0 approvers): 59 bytes
//!
//! INVARIANTS:
//! - Field order is CONSENSUS-RELEVANT - changing it is a hard fork
//! - All integers are big-endian (standard for blockchain protocols)
//! - Approvers must be sorted canonically (enforced by ApprovalGate::new)
//!
//! FAILURE MODES:
//! - Invalid version byte causes decode failure
//! - Invalid gate type byte causes decode failure
//! - Truncated input causes decode failure
//! - Trailing bytes cause decode failure

use novai_ai_entities::gates::{ApprovalGate, GateId, GateType, MAX_APPROVERS};
use novai_types::Address;

/// Version byte for ApprovalGate encoding.
pub const APPROVAL_GATE_V1: u8 = 0x01;

/// Minimum encoded size of ApprovalGate v1 (with 0 approvers).
///
/// Layout: version(1) + gate_id(32) + gate_type(1) + count(4) + threshold(4)
///         + timelock(8) + expiry(8) + flags(1) = 59 bytes
pub const APPROVAL_GATE_V1_MIN_SIZE: usize = 59;

/// Errors during gate encoding/decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateCodecError {
    /// Input buffer is too short.
    BufferTooShort { expected: usize, actual: usize },

    /// Unknown or unsupported version byte.
    InvalidVersion(u8),

    /// Invalid gate type discriminant.
    InvalidGateType(u8),

    /// Too many approvers (exceeds MAX_APPROVERS).
    TooManyApprovers { count: u32, max: usize },

    /// Trailing bytes after decoding complete.
    TrailingBytes { count: usize },

    /// Gate validation failed after decoding.
    ValidationFailed(String),
}

impl std::fmt::Display for GateCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateCodecError::BufferTooShort { expected, actual } => {
                write!(
                    f,
                    "buffer too short: need {} bytes, got {}",
                    expected, actual
                )
            }
            GateCodecError::InvalidVersion(v) => {
                write!(f, "invalid version byte: 0x{:02x}", v)
            }
            GateCodecError::InvalidGateType(t) => {
                write!(f, "invalid gate type: {}", t)
            }
            GateCodecError::TooManyApprovers { count, max } => {
                write!(f, "too many approvers: {} exceeds max {}", count, max)
            }
            GateCodecError::TrailingBytes { count } => {
                write!(f, "trailing bytes: {} unexpected bytes", count)
            }
            GateCodecError::ValidationFailed(msg) => {
                write!(f, "gate validation failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for GateCodecError {}

/// Encode an ApprovalGate to canonical bytes (big-endian).
///
/// # Wire Format
///
/// ```text
/// [version:1][gate_id:32][gate_type:1][approvers_count:4][approvers:32*N]
/// [threshold:4][timelock_blocks:8][expiry_blocks:8][flags:1]
/// ```
///
/// All multi-byte integers are encoded in big-endian format.
///
/// # Examples
///
/// ```
/// use novai_ai_entities::gates::{ApprovalGate, GateType};
/// use novai_codec::gate_codec::encode_approval_gate_v1;
///
/// let gate = ApprovalGate::new(
///     GateType::Multisig,
///     vec![[0x01u8; 32], [0x02u8; 32]],
///     2, 100, 1000, false, false,
/// ).unwrap();
///
/// let encoded = encode_approval_gate_v1(&gate);
/// assert!(encoded.len() > 59); // Minimum size + 2 approvers
/// assert_eq!(encoded[0], 0x01); // Version byte
/// ```
#[must_use]
pub fn encode_approval_gate_v1(gate: &ApprovalGate) -> Vec<u8> {
    // Pre-calculate size for efficiency
    let approver_bytes = gate.required_approvers.len() * 32;
    let total_size = APPROVAL_GATE_V1_MIN_SIZE + approver_bytes;
    let mut buf = Vec::with_capacity(total_size);

    // Version
    buf.push(APPROVAL_GATE_V1);

    // Gate ID (32 bytes)
    buf.extend_from_slice(&gate.gate_id);

    // Gate type (1 byte)
    buf.push(gate.gate_type.to_byte());

    // Approvers count (4 bytes, big-endian)
    #[allow(clippy::cast_possible_truncation)]
    let approvers_count = gate.required_approvers.len() as u32;
    buf.extend_from_slice(&approvers_count.to_be_bytes());

    // Approvers (32 bytes each, already sorted by ApprovalGate::new)
    for approver in &gate.required_approvers {
        buf.extend_from_slice(approver);
    }

    // Threshold (4 bytes, big-endian)
    buf.extend_from_slice(&gate.threshold.to_be_bytes());

    // Timelock blocks (8 bytes, big-endian)
    buf.extend_from_slice(&gate.timelock_blocks.to_be_bytes());

    // Expiry blocks (8 bytes, big-endian)
    buf.extend_from_slice(&gate.expiry_blocks.to_be_bytes());

    // Flags (1 byte)
    buf.push(gate.flags());

    debug_assert_eq!(buf.len(), total_size);
    buf
}

/// Decode an ApprovalGate from canonical bytes (big-endian).
///
/// # Errors
///
/// Returns `GateCodecError` if:
/// - Buffer is too short
/// - Version byte is unsupported
/// - Gate type is invalid
/// - Too many approvers
/// - Trailing bytes present
/// - Decoded gate fails validation
///
/// # Examples
///
/// ```
/// use novai_ai_entities::gates::{ApprovalGate, GateType};
/// use novai_codec::gate_codec::{encode_approval_gate_v1, decode_approval_gate_v1};
///
/// let gate = ApprovalGate::new(
///     GateType::Threshold,
///     vec![[0x01u8; 32], [0x02u8; 32], [0x03u8; 32]],
///     2, 50, 500, true, false,
/// ).unwrap();
///
/// let encoded = encode_approval_gate_v1(&gate);
/// let decoded = decode_approval_gate_v1(&encoded).unwrap();
///
/// assert_eq!(gate.gate_id, decoded.gate_id);
/// assert_eq!(gate.threshold, decoded.threshold);
/// ```
pub fn decode_approval_gate_v1(input: &[u8]) -> Result<ApprovalGate, GateCodecError> {
    if input.len() < APPROVAL_GATE_V1_MIN_SIZE {
        return Err(GateCodecError::BufferTooShort {
            expected: APPROVAL_GATE_V1_MIN_SIZE,
            actual: input.len(),
        });
    }

    let mut cursor = 0;

    // Version (1 byte)
    let version = input[cursor];
    cursor += 1;
    if version != APPROVAL_GATE_V1 {
        return Err(GateCodecError::InvalidVersion(version));
    }

    // Gate ID (32 bytes)
    let mut gate_id: GateId = [0u8; 32];
    gate_id.copy_from_slice(&input[cursor..cursor + 32]);
    cursor += 32;

    // Gate type (1 byte)
    let gate_type_byte = input[cursor];
    cursor += 1;
    let gate_type = GateType::from_byte(gate_type_byte)
        .ok_or(GateCodecError::InvalidGateType(gate_type_byte))?;

    // Approvers count (4 bytes, big-endian)
    let approvers_count = u32::from_be_bytes([
        input[cursor],
        input[cursor + 1],
        input[cursor + 2],
        input[cursor + 3],
    ]);
    cursor += 4;

    // Validate approvers count
    if approvers_count as usize > MAX_APPROVERS {
        return Err(GateCodecError::TooManyApprovers {
            count: approvers_count,
            max: MAX_APPROVERS,
        });
    }

    // Calculate required remaining bytes
    let approver_bytes = approvers_count as usize * 32;
    let remaining_fixed = 4 + 8 + 8 + 1; // threshold + timelock + expiry + flags
    let expected_remaining = approver_bytes + remaining_fixed;

    if input.len() < cursor + expected_remaining {
        return Err(GateCodecError::BufferTooShort {
            expected: cursor + expected_remaining,
            actual: input.len(),
        });
    }

    // Approvers (32 bytes each)
    let mut required_approvers: Vec<Address> = Vec::with_capacity(approvers_count as usize);
    for _ in 0..approvers_count {
        let mut addr: Address = [0u8; 32];
        addr.copy_from_slice(&input[cursor..cursor + 32]);
        cursor += 32;
        required_approvers.push(addr);
    }

    // Threshold (4 bytes, big-endian)
    let threshold = u32::from_be_bytes([
        input[cursor],
        input[cursor + 1],
        input[cursor + 2],
        input[cursor + 3],
    ]);
    cursor += 4;

    // Timelock blocks (8 bytes, big-endian)
    let timelock_blocks = u64::from_be_bytes([
        input[cursor],
        input[cursor + 1],
        input[cursor + 2],
        input[cursor + 3],
        input[cursor + 4],
        input[cursor + 5],
        input[cursor + 6],
        input[cursor + 7],
    ]);
    cursor += 8;

    // Expiry blocks (8 bytes, big-endian)
    let expiry_blocks = u64::from_be_bytes([
        input[cursor],
        input[cursor + 1],
        input[cursor + 2],
        input[cursor + 3],
        input[cursor + 4],
        input[cursor + 5],
        input[cursor + 6],
        input[cursor + 7],
    ]);
    cursor += 8;

    // Flags (1 byte)
    let flags = input[cursor];
    cursor += 1;
    let (veto_enabled, freeze_enabled) = ApprovalGate::unpack_flags(flags);

    // Check for trailing bytes
    if cursor != input.len() {
        return Err(GateCodecError::TrailingBytes {
            count: input.len() - cursor,
        });
    }

    // Construct gate directly (approvers already sorted from encoding)
    let gate = ApprovalGate {
        gate_id,
        gate_type,
        required_approvers,
        threshold,
        timelock_blocks,
        expiry_blocks,
        veto_enabled,
        freeze_enabled,
    };

    // Validate the decoded gate
    gate.validate()
        .map_err(|e| GateCodecError::ValidationFailed(e.to_string()))?;

    Ok(gate)
}

/// Calculate the encoded size of an ApprovalGate.
///
/// Useful for pre-allocating buffers or validating message sizes.
///
/// # Examples
///
/// ```
/// use novai_ai_entities::gates::{ApprovalGate, GateType};
/// use novai_codec::gate_codec::{encoded_gate_size, encode_approval_gate_v1};
///
/// let gate = ApprovalGate::new(
///     GateType::Multisig,
///     vec![[0x01u8; 32], [0x02u8; 32]],
///     2, 100, 1000, false, false,
/// ).unwrap();
///
/// let predicted_size = encoded_gate_size(&gate);
/// let actual_size = encode_approval_gate_v1(&gate).len();
///
/// assert_eq!(predicted_size, actual_size);
/// ```
#[must_use]
pub fn encoded_gate_size(gate: &ApprovalGate) -> usize {
    APPROVAL_GATE_V1_MIN_SIZE + (gate.required_approvers.len() * 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_approvers(count: usize) -> Vec<Address> {
        (0..count)
            .map(|i| {
                let mut addr = [0u8; 32];
                addr[0] = i as u8;
                addr[1] = (i >> 8) as u8;
                addr
            })
            .collect()
    }

    #[test]
    fn encode_starts_with_version() {
        let gate = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(2),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        let encoded = encode_approval_gate_v1(&gate);
        assert_eq!(encoded[0], APPROVAL_GATE_V1);
    }

    #[test]
    fn encode_produces_correct_size() {
        let gate = ApprovalGate::new(
            GateType::Threshold,
            test_approvers(3),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        let encoded = encode_approval_gate_v1(&gate);
        let expected_size = APPROVAL_GATE_V1_MIN_SIZE + (3 * 32);

        assert_eq!(encoded.len(), expected_size);
        assert_eq!(encoded.len(), encoded_gate_size(&gate));
    }

    #[test]
    fn roundtrip_multisig_gate() {
        let gate = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(3),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        let encoded = encode_approval_gate_v1(&gate);
        let decoded = decode_approval_gate_v1(&encoded).unwrap();

        assert_eq!(gate.gate_id, decoded.gate_id);
        assert_eq!(gate.gate_type, decoded.gate_type);
        assert_eq!(gate.required_approvers, decoded.required_approvers);
        assert_eq!(gate.threshold, decoded.threshold);
        assert_eq!(gate.timelock_blocks, decoded.timelock_blocks);
        assert_eq!(gate.expiry_blocks, decoded.expiry_blocks);
        assert_eq!(gate.veto_enabled, decoded.veto_enabled);
        assert_eq!(gate.freeze_enabled, decoded.freeze_enabled);
    }

    #[test]
    fn roundtrip_threshold_gate() {
        let gate = ApprovalGate::new(
            GateType::Threshold,
            test_approvers(5),
            3,
            200,
            2000,
            true,
            true,
        )
        .unwrap();

        let encoded = encode_approval_gate_v1(&gate);
        let decoded = decode_approval_gate_v1(&encoded).unwrap();

        assert_eq!(gate.gate_id, decoded.gate_id);
        assert_eq!(gate.gate_type, decoded.gate_type);
        assert_eq!(gate.threshold, decoded.threshold);
        assert_eq!(gate.veto_enabled, decoded.veto_enabled);
        assert_eq!(gate.freeze_enabled, decoded.freeze_enabled);
    }

    #[test]
    fn roundtrip_timelock_only_gate() {
        let gate = ApprovalGate::new(
            GateType::TimelockOnly,
            vec![], // No approvers
            0,
            500,
            5000,
            false,
            true,
        )
        .unwrap();

        let encoded = encode_approval_gate_v1(&gate);
        assert_eq!(encoded.len(), APPROVAL_GATE_V1_MIN_SIZE);

        let decoded = decode_approval_gate_v1(&encoded).unwrap();
        assert_eq!(gate.gate_id, decoded.gate_id);
        assert_eq!(gate.gate_type, decoded.gate_type);
        assert!(decoded.required_approvers.is_empty());
    }

    #[test]
    fn encoding_is_deterministic() {
        let gate = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(3),
            2,
            100,
            1000,
            true,
            false,
        )
        .unwrap();

        let encoded1 = encode_approval_gate_v1(&gate);
        let encoded2 = encode_approval_gate_v1(&gate);

        assert_eq!(encoded1, encoded2, "Encoding must be deterministic");
    }

    #[test]
    fn decode_rejects_short_buffer() {
        let short = vec![0u8; 10];
        let result = decode_approval_gate_v1(&short);

        assert!(matches!(result, Err(GateCodecError::BufferTooShort { .. })));
    }

    #[test]
    fn decode_rejects_invalid_version() {
        let gate = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(2),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        let mut encoded = encode_approval_gate_v1(&gate);
        encoded[0] = 0xFF; // Invalid version

        let result = decode_approval_gate_v1(&encoded);
        assert!(matches!(result, Err(GateCodecError::InvalidVersion(0xFF))));
    }

    #[test]
    fn decode_rejects_invalid_gate_type() {
        let gate = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(2),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        let mut encoded = encode_approval_gate_v1(&gate);
        encoded[33] = 0xFF; // Invalid gate type at offset 33

        let result = decode_approval_gate_v1(&encoded);
        assert!(matches!(result, Err(GateCodecError::InvalidGateType(0xFF))));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let gate =
            ApprovalGate::new(GateType::TimelockOnly, vec![], 0, 100, 1000, false, false).unwrap();

        let mut encoded = encode_approval_gate_v1(&gate);
        encoded.push(0x00); // Add trailing byte

        let result = decode_approval_gate_v1(&encoded);
        assert!(matches!(
            result,
            Err(GateCodecError::TrailingBytes { count: 1 })
        ));
    }

    #[test]
    fn decode_rejects_too_many_approvers() {
        // Manually craft a complete buffer but with too many approvers count.
        // The buffer must be at least APPROVAL_GATE_V1_MIN_SIZE to pass initial check.
        let mut buf = vec![APPROVAL_GATE_V1]; // version (1)
        buf.extend_from_slice(&[0u8; 32]); // gate_id (32)
        buf.push(GateType::Multisig.to_byte()); // gate_type (1)

        // Use a count just over MAX_APPROVERS - this will be checked
        // right after reading, before trying to read the approvers
        let huge_count = (MAX_APPROVERS + 1) as u32;
        buf.extend_from_slice(&huge_count.to_be_bytes()); // count (4)

        // Add the remaining fixed fields to make it look valid length-wise
        // for the initial check (but it will fail at approver count validation)
        buf.extend_from_slice(&1u32.to_be_bytes()); // threshold (4)
        buf.extend_from_slice(&100u64.to_be_bytes()); // timelock (8)
        buf.extend_from_slice(&1000u64.to_be_bytes()); // expiry (8)
        buf.push(0x00); // flags (1)

        assert_eq!(buf.len(), APPROVAL_GATE_V1_MIN_SIZE);

        let result = decode_approval_gate_v1(&buf);
        assert!(
            matches!(result, Err(GateCodecError::TooManyApprovers { .. })),
            "Expected TooManyApprovers error, got {:?}",
            result
        );
    }

    #[test]
    fn roundtrip_preserves_flags() {
        // Test all flag combinations
        for veto in [false, true] {
            for freeze in [false, true] {
                let gate = ApprovalGate::new(
                    GateType::Multisig,
                    test_approvers(2),
                    2,
                    100,
                    1000,
                    veto,
                    freeze,
                )
                .unwrap();

                let encoded = encode_approval_gate_v1(&gate);
                let decoded = decode_approval_gate_v1(&encoded).unwrap();

                assert_eq!(
                    veto, decoded.veto_enabled,
                    "veto flag not preserved for veto={}, freeze={}",
                    veto, freeze
                );
                assert_eq!(
                    freeze, decoded.freeze_enabled,
                    "freeze flag not preserved for veto={}, freeze={}",
                    veto, freeze
                );
            }
        }
    }

    #[test]
    fn encoded_gate_size_matches_actual() {
        for approver_count in [0, 1, 5, 10, 50] {
            let gate_type = if approver_count == 0 {
                GateType::TimelockOnly
            } else {
                GateType::Multisig
            };

            let threshold = if approver_count == 0 {
                0
            } else {
                approver_count.min(1)
            };

            let gate = ApprovalGate::new(
                gate_type,
                test_approvers(approver_count),
                threshold as u32,
                100,
                1000,
                false,
                false,
            )
            .unwrap();

            let predicted = encoded_gate_size(&gate);
            let actual = encode_approval_gate_v1(&gate).len();

            assert_eq!(
                predicted, actual,
                "Size mismatch for {} approvers",
                approver_count
            );
        }
    }
}
