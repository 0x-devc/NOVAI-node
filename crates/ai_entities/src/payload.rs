//! Signal payload format for off-chain storage (Week 15).
//!
//! PURPOSE: Defines the structured format for AI signal payloads that are
//! stored off-chain and referenced by commitment hash on-chain.
//!
//! INVARIANTS:
//! - All string fields are valid UTF-8
//! - String fields are limited to MAX_STRING_LENGTH (1024 bytes)
//! - output_data is limited to MAX_OUTPUT_DATA_LENGTH (10MB)
//! - Encoding is canonical: version byte + LE u32 length-prefixed fields
//!
//! FAILURE MODES:
//! - decode() returns None for invalid version, malformed data, or trailing bytes
//! - Field length violations are rejected at decode time

use crate::artifact_hash;

/// Maximum length for string fields (model_id, model_version, input_summary, explanation).
pub const MAX_STRING_LENGTH: usize = 1024;

/// Maximum length for output_data field (10MB).
pub const MAX_OUTPUT_DATA_LENGTH: usize = 10 * 1024 * 1024;

/// Current payload format version.
const PAYLOAD_VERSION: u8 = 1;

/// Structured payload for AI signals stored off-chain.
///
/// This is the detailed data that accompanies an on-chain signal commitment.
/// The commitment hash is computed over the canonical encoding of this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalPayload {
    /// Identifier of the AI model that generated this signal.
    pub model_id: String,
    /// Version of the AI model.
    pub model_version: String,
    /// Summary of the input data used to generate the signal.
    pub input_summary: String,
    /// Raw output data from the model (binary).
    pub output_data: Vec<u8>,
    /// Human-readable explanation of the signal.
    pub explanation: String,
}

impl SignalPayload {
    /// Create a new signal payload.
    pub fn new(
        model_id: String,
        model_version: String,
        input_summary: String,
        output_data: Vec<u8>,
        explanation: String,
    ) -> Self {
        Self {
            model_id,
            model_version,
            input_summary,
            output_data,
            explanation,
        }
    }

    /// Encode the payload to canonical bytes.
    ///
    /// Format:
    /// - 1 byte: version (currently 1)
    /// - 4 bytes LE: model_id length
    /// - N bytes: model_id UTF-8 bytes
    /// - 4 bytes LE: model_version length
    /// - N bytes: model_version UTF-8 bytes
    /// - 4 bytes LE: input_summary length
    /// - N bytes: input_summary UTF-8 bytes
    /// - 4 bytes LE: output_data length
    /// - N bytes: output_data bytes
    /// - 4 bytes LE: explanation length
    /// - N bytes: explanation UTF-8 bytes
    pub fn encode(&self) -> Vec<u8> {
        let total_len = 1  // version
            + 4 + self.model_id.len()
            + 4 + self.model_version.len()
            + 4 + self.input_summary.len()
            + 4 + self.output_data.len()
            + 4 + self.explanation.len();

        let mut buf = Vec::with_capacity(total_len);

        // Version byte
        buf.push(PAYLOAD_VERSION);

        // model_id
        buf.extend_from_slice(&(self.model_id.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.model_id.as_bytes());

        // model_version
        buf.extend_from_slice(&(self.model_version.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.model_version.as_bytes());

        // input_summary
        buf.extend_from_slice(&(self.input_summary.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.input_summary.as_bytes());

        // output_data
        buf.extend_from_slice(&(self.output_data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.output_data);

        // explanation
        buf.extend_from_slice(&(self.explanation.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.explanation.as_bytes());

        buf
    }

    /// Decode a payload from bytes.
    ///
    /// Returns None if:
    /// - Version byte is not 1
    /// - Any length prefix exceeds limits
    /// - Any string field is not valid UTF-8
    /// - There are trailing bytes after the payload
    /// - Data is truncated
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let mut pos = 0;

        // Version check
        let version = data[pos];
        if version != PAYLOAD_VERSION {
            return None;
        }
        pos += 1;

        // Helper to read a length-prefixed string
        let read_string = |pos: &mut usize, max_len: usize| -> Option<String> {
            if *pos + 4 > data.len() {
                return None;
            }
            let len = u32::from_le_bytes(data[*pos..*pos + 4].try_into().ok()?) as usize;
            *pos += 4;

            if len > max_len {
                return None;
            }
            if *pos + len > data.len() {
                return None;
            }
            let s = std::str::from_utf8(&data[*pos..*pos + len])
                .ok()?
                .to_string();
            *pos += len;
            Some(s)
        };

        // Helper to read a length-prefixed byte array
        let read_bytes = |pos: &mut usize, max_len: usize| -> Option<Vec<u8>> {
            if *pos + 4 > data.len() {
                return None;
            }
            let len = u32::from_le_bytes(data[*pos..*pos + 4].try_into().ok()?) as usize;
            *pos += 4;

            if len > max_len {
                return None;
            }
            if *pos + len > data.len() {
                return None;
            }
            let bytes = data[*pos..*pos + len].to_vec();
            *pos += len;
            Some(bytes)
        };

        // Read fields
        let model_id = read_string(&mut pos, MAX_STRING_LENGTH)?;
        let model_version = read_string(&mut pos, MAX_STRING_LENGTH)?;
        let input_summary = read_string(&mut pos, MAX_STRING_LENGTH)?;
        let output_data = read_bytes(&mut pos, MAX_OUTPUT_DATA_LENGTH)?;
        let explanation = read_string(&mut pos, MAX_STRING_LENGTH)?;

        // Reject trailing bytes
        if pos != data.len() {
            return None;
        }

        Some(Self {
            model_id,
            model_version,
            input_summary,
            output_data,
            explanation,
        })
    }

    /// Compute the content-addressed hash of this payload.
    ///
    /// This hash is used as the commitment stored on-chain.
    pub fn compute_hash(&self) -> [u8; 32] {
        artifact_hash(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> SignalPayload {
        SignalPayload::new(
            "gpt-4".to_string(),
            "1.0.0".to_string(),
            "BTC/USD price query".to_string(),
            vec![0x42, 0x43, 0x44],
            "Model predicts upward trend".to_string(),
        )
    }

    #[test]
    fn encode_decode_roundtrip() {
        let payload = sample_payload();
        let encoded = payload.encode();
        let decoded = SignalPayload::decode(&encoded).expect("decode should succeed");
        assert_eq!(payload, decoded);
    }

    #[test]
    fn decode_empty_fails() {
        assert!(SignalPayload::decode(&[]).is_none());
    }

    #[test]
    fn decode_wrong_version_fails() {
        let mut encoded = sample_payload().encode();
        encoded[0] = 0xFF; // Invalid version
        assert!(SignalPayload::decode(&encoded).is_none());
    }

    #[test]
    fn decode_truncated_fails() {
        let encoded = sample_payload().encode();
        // Truncate in the middle
        assert!(SignalPayload::decode(&encoded[..encoded.len() / 2]).is_none());
    }

    #[test]
    fn decode_trailing_bytes_fails() {
        let mut encoded = sample_payload().encode();
        encoded.push(0xFF); // Extra byte
        assert!(SignalPayload::decode(&encoded).is_none());
    }

    #[test]
    fn decode_invalid_utf8_fails() {
        let payload = sample_payload();
        let mut encoded = payload.encode();
        // Corrupt the model_id field (after version byte and length prefix)
        // Position 5 is the start of model_id string content
        if encoded.len() > 5 {
            encoded[5] = 0xFF; // Invalid UTF-8 byte
        }
        assert!(SignalPayload::decode(&encoded).is_none());
    }

    #[test]
    fn decode_string_too_long_fails() {
        // Create encoded data with a string length exceeding MAX_STRING_LENGTH
        let mut data = vec![PAYLOAD_VERSION];
        // model_id length = MAX_STRING_LENGTH + 1
        data.extend_from_slice(&((MAX_STRING_LENGTH + 1) as u32).to_le_bytes());
        // We don't need to add the actual content since length check should fail
        data.extend(vec![0x41; MAX_STRING_LENGTH + 1]); // 'A' bytes

        assert!(SignalPayload::decode(&data).is_none());
    }

    #[test]
    fn decode_output_data_too_long_fails() {
        // We can't actually create 10MB+ of test data, but we can test the length check
        // by crafting a header that claims a length exceeding MAX_OUTPUT_DATA_LENGTH
        let mut data = vec![PAYLOAD_VERSION];

        // Empty model_id
        data.extend_from_slice(&0u32.to_le_bytes());
        // Empty model_version
        data.extend_from_slice(&0u32.to_le_bytes());
        // Empty input_summary
        data.extend_from_slice(&0u32.to_le_bytes());
        // output_data length = MAX_OUTPUT_DATA_LENGTH + 1
        data.extend_from_slice(&((MAX_OUTPUT_DATA_LENGTH + 1) as u32).to_le_bytes());

        assert!(SignalPayload::decode(&data).is_none());
    }

    #[test]
    fn compute_hash_is_deterministic() {
        let payload = sample_payload();
        let hash1 = payload.compute_hash();
        let hash2 = payload.compute_hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn compute_hash_changes_with_content() {
        let payload1 = sample_payload();
        let mut payload2 = sample_payload();
        payload2.model_id = "different-model".to_string();

        assert_ne!(payload1.compute_hash(), payload2.compute_hash());
    }

    #[test]
    fn empty_fields_encode_decode() {
        let payload = SignalPayload::new(
            String::new(),
            String::new(),
            String::new(),
            Vec::new(),
            String::new(),
        );
        let encoded = payload.encode();
        let decoded = SignalPayload::decode(&encoded).expect("decode should succeed");
        assert_eq!(payload, decoded);
    }

    #[test]
    fn max_length_strings_work() {
        let long_string = "x".repeat(MAX_STRING_LENGTH);
        let payload = SignalPayload::new(
            long_string.clone(),
            long_string.clone(),
            long_string.clone(),
            vec![0x42],
            long_string,
        );
        let encoded = payload.encode();
        let decoded = SignalPayload::decode(&encoded).expect("decode should succeed");
        assert_eq!(payload, decoded);
    }

    // Golden vector test to lock encoding format
    #[test]
    fn golden_vector_encoding() {
        let payload = SignalPayload::new(
            "test".to_string(),
            "1.0".to_string(),
            "input".to_string(),
            vec![0xDE, 0xAD],
            "explain".to_string(),
        );

        let encoded = payload.encode();

        // Expected format:
        // 01                         - version
        // 04 00 00 00 74 65 73 74    - model_id: "test"
        // 03 00 00 00 31 2e 30       - model_version: "1.0"
        // 05 00 00 00 69 6e 70 75 74 - input_summary: "input"
        // 02 00 00 00 de ad          - output_data: [0xDE, 0xAD]
        // 07 00 00 00 65 78 70 6c 61 69 6e - explanation: "explain"

        let expected = vec![
            0x01, // version
            0x04, 0x00, 0x00, 0x00, 0x74, 0x65, 0x73, 0x74, // "test"
            0x03, 0x00, 0x00, 0x00, 0x31, 0x2e, 0x30, // "1.0"
            0x05, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x70, 0x75, 0x74, // "input"
            0x02, 0x00, 0x00, 0x00, 0xde, 0xad, // [0xDE, 0xAD]
            0x07, 0x00, 0x00, 0x00, 0x65, 0x78, 0x70, 0x6c, 0x61, 0x69, 0x6e, // "explain"
        ];

        assert_eq!(encoded, expected, "Encoding format must not change");
    }

    #[test]
    fn golden_vector_hash() {
        let payload = SignalPayload::new(
            "test".to_string(),
            "1.0".to_string(),
            "input".to_string(),
            vec![0xDE, 0xAD],
            "explain".to_string(),
        );

        let hash = payload.compute_hash();

        // This hash is computed from the golden vector encoding above
        // using artifact_hash (blake3 with NOVAI_ARTIFACT_V1 domain separator)
        let expected_hex = "82893ff1a1d3532a596390fa5753a8e207c5f64077812fb1017e5d427badccbb";
        let actual_hex = hex::encode(hash);

        assert_eq!(actual_hex, expected_hex, "Hash must not change");
    }
}
