//! Deterministic leader selection for consensus.
//!
//! The leader schedule is deterministic and round-robin based on validator ordering.
//! Leader selection: leader(height, round) = validators[(height + round) % n]

use novai_types::Address;

/// Validator set errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatorSetError {
    /// Validator set is empty.
    Empty,
    /// Validator set has fewer than 4 validators.
    TooFew,
    /// Validator set size doesn't satisfy n = 3f + 1.
    InvalidSize,
    /// Validator set contains duplicate addresses.
    DuplicateValidator,
}

/// Validator set for consensus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorSet {
    /// Validators sorted by Address (lexicographic order).
    validators: Vec<Address>,
}

impl ValidatorSet {
    /// Create a new validator set from addresses.
    ///
    /// Validators are automatically sorted by Address to ensure determinism.
    ///
    /// # Errors
    /// Returns error if:
    /// - n < 4 (insufficient for BFT)
    /// - n != 3f + 1 for some integer f
    /// - Duplicate validators exist
    pub fn new(mut addresses: Vec<Address>) -> Result<Self, ValidatorSetError> {
        // Check minimum size
        if addresses.is_empty() {
            return Err(ValidatorSetError::Empty);
        }
        if addresses.len() < 4 {
            return Err(ValidatorSetError::TooFew);
        }

        // Check n = 3f + 1 constraint
        let n = addresses.len();
        if !(n - 1).is_multiple_of(3) {
            return Err(ValidatorSetError::InvalidSize);
        }

        // Sort by lexicographic byte order (deterministic)
        addresses.sort_unstable();

        // Check for duplicates (adjacent after sorting)
        for i in 1..addresses.len() {
            if addresses[i] == addresses[i - 1] {
                return Err(ValidatorSetError::DuplicateValidator);
            }
        }

        Ok(Self {
            validators: addresses,
        })
    }

    /// Get the number of validators.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.validators.len()
    }

    /// Check if the validator set is empty.
    ///
    /// Note: This will always return false for valid validator sets,
    /// since construction requires n >= 4.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    /// Get all validator addresses (in sorted order).
    #[must_use]
    pub fn validators(&self) -> &[Address] {
        &self.validators
    }

    /// Check if an address is a validator.
    #[must_use]
    pub fn contains(&self, addr: &Address) -> bool {
        self.validators.binary_search(addr).is_ok()
    }

    /// Compute the leader for a given height and round.
    ///
    /// Formula: leader(height, round) = validators[(height + round) % n]
    ///
    /// # Panics
    /// Panics if the validator set is empty (should never happen for valid sets).
    #[must_use]
    pub fn leader(&self, height: u64, round: u64) -> Address {
        assert!(!self.validators.is_empty(), "validator set cannot be empty");

        let index = height.wrapping_add(round) % (self.validators.len() as u64);
        #[allow(clippy::cast_possible_truncation)]
        let idx = index as usize; // Safe: index < validators.len() < usize::MAX
        self.validators[idx]
    }

    /// Compute the quorum threshold (2f + 1).
    ///
    /// For n = 3f + 1 validators, quorum = 2f + 1.
    #[must_use]
    pub const fn quorum_threshold(&self) -> usize {
        let n = self.validators.len();
        // 2f + 1 = 2 * ((n - 1) / 3) + 1
        2 * ((n - 1) / 3) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validator_set_rejects_too_few() {
        // n = 3 is too few (need n >= 4)
        let result = ValidatorSet::new(vec![[0x01; 32], [0x02; 32], [0x03; 32]]);
        assert_eq!(result, Err(ValidatorSetError::TooFew));
    }

    #[test]
    fn validator_set_rejects_invalid_size() {
        // n = 5 doesn't satisfy n = 3f + 1
        let result = ValidatorSet::new(vec![
            [0x01; 32], [0x02; 32], [0x03; 32], [0x04; 32], [0x05; 32],
        ]);
        assert_eq!(result, Err(ValidatorSetError::InvalidSize));
    }

    #[test]
    fn validator_set_rejects_duplicates() {
        let result = ValidatorSet::new(vec![[0x01; 32], [0x01; 32], [0x02; 32], [0x03; 32]]);
        assert_eq!(result, Err(ValidatorSetError::DuplicateValidator));
    }

    #[test]
    fn validator_set_sorts_addresses() {
        let addr_a = [0xaa; 32];
        let addr_b = [0xbb; 32];
        let addr_c = [0xcc; 32];
        let addr_d = [0xdd; 32];

        // Create in reverse order
        let vs = ValidatorSet::new(vec![addr_d, addr_c, addr_a, addr_b]).unwrap();

        // Should be sorted
        assert_eq!(vs.validators(), &[addr_a, addr_b, addr_c, addr_d]);
    }

    #[test]
    fn leader_deterministic() {
        let addr_1 = [0x01; 32];
        let addr_2 = [0x02; 32];
        let addr_3 = [0x03; 32];
        let addr_4 = [0x04; 32];

        let vs = ValidatorSet::new(vec![addr_1, addr_2, addr_3, addr_4]).unwrap();

        // Height 0, Round 0 -> index 0
        assert_eq!(vs.leader(0, 0), addr_1);

        // Height 0, Round 1 -> index 1
        assert_eq!(vs.leader(0, 1), addr_2);

        // Height 1, Round 0 -> index 1
        assert_eq!(vs.leader(1, 0), addr_2);

        // Height 2, Round 2 -> (2+2) % 4 = 0
        assert_eq!(vs.leader(2, 2), addr_1);

        // Height 10, Round 5 -> (10+5) % 4 = 3
        assert_eq!(vs.leader(10, 5), addr_4);
    }

    #[test]
    fn quorum_threshold_correct() {
        // n = 4 -> f = 1 -> quorum = 3
        let vs4 = ValidatorSet::new(vec![[0x01; 32], [0x02; 32], [0x03; 32], [0x04; 32]]).unwrap();
        assert_eq!(vs4.quorum_threshold(), 3);

        // n = 7 -> f = 2 -> quorum = 5
        let vs7 = ValidatorSet::new(vec![
            [0x01; 32], [0x02; 32], [0x03; 32], [0x04; 32], [0x05; 32], [0x06; 32], [0x07; 32],
        ])
        .unwrap();
        assert_eq!(vs7.quorum_threshold(), 5);

        // n = 10 -> f = 3 -> quorum = 7
        let vs10 = ValidatorSet::new(vec![
            [0x01; 32], [0x02; 32], [0x03; 32], [0x04; 32], [0x05; 32], [0x06; 32], [0x07; 32],
            [0x08; 32], [0x09; 32], [0x0a; 32],
        ])
        .unwrap();
        assert_eq!(vs10.quorum_threshold(), 7);
    }

    #[test]
    fn contains_check() {
        let addr_1 = [0x01; 32];
        let addr_2 = [0x02; 32];
        let addr_3 = [0x03; 32];
        let addr_4 = [0x04; 32];
        let addr_5 = [0x05; 32];

        let vs = ValidatorSet::new(vec![addr_1, addr_2, addr_3, addr_4]).unwrap();

        assert!(vs.contains(&addr_1));
        assert!(vs.contains(&addr_2));
        assert!(!vs.contains(&addr_5));
    }
}
