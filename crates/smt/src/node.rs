use crate::hash::Hash32;

/// Child pointer: either a hash to another node, or an empty subtree at some height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeChild {
    Hash(Hash32),
    Empty { height: u16 },
}

/// A binary SMT node.
/// This node conceptually sits at some height (tracked by caller; not stored here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub left: NodeChild,
    pub right: NodeChild,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEncodingError {
    BadLength { expected: usize, got: usize },
    BadTag { got: u8 },
}

/// Canonical encoding:
/// [tag:1][left:33][right:33]
///
/// Child encoding:
/// - Hash:  [0x01][32 bytes]
/// - Empty: [0x00][height:u16 be][padding 29 zeros]
///
/// Total = 1 + 33 + 33 = 67 bytes.
impl Node {
    pub const ENCODED_LEN: usize = 67;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[0] = 0xA1; // node tag (arbitrary fixed value)

        encode_child(&mut out[1..34], &self.left);
        encode_child(&mut out[34..67], &self.right);

        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Node, NodeEncodingError> {
        if bytes.len() != Self::ENCODED_LEN {
            return Err(NodeEncodingError::BadLength {
                expected: Self::ENCODED_LEN,
                got: bytes.len(),
            });
        }
        if bytes[0] != 0xA1 {
            return Err(NodeEncodingError::BadTag { got: bytes[0] });
        }
        let left = decode_child(&bytes[1..34])?;
        let right = decode_child(&bytes[34..67])?;
        Ok(Node { left, right })
    }
}

fn encode_child(dst: &mut [u8], c: &NodeChild) {
    debug_assert_eq!(dst.len(), 33);
    match c {
        NodeChild::Hash(h) => {
            dst[0] = 0x01;
            dst[1..33].copy_from_slice(h);
        }
        NodeChild::Empty { height } => {
            dst[0] = 0x00;
            dst[1..3].copy_from_slice(&height.to_be_bytes());
            // remaining bytes are already zero
        }
    }
}

fn decode_child(src: &[u8]) -> Result<NodeChild, NodeEncodingError> {
    debug_assert_eq!(src.len(), 33);
    match src[0] {
        0x01 => {
            let mut h = [0u8; 32];
            h.copy_from_slice(&src[1..33]);
            Ok(NodeChild::Hash(h))
        }
        0x00 => {
            let mut hb = [0u8; 2];
            hb.copy_from_slice(&src[1..3]);
            Ok(NodeChild::Empty {
                height: u16::from_be_bytes(hb),
            })
        }
        other => Err(NodeEncodingError::BadTag { got: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_roundtrip_hash_children() {
        let h1: Hash32 = [0x10u8; 32];
        let h2: Hash32 = [0x20u8; 32];
        let n = Node {
            left: NodeChild::Hash(h1),
            right: NodeChild::Hash(h2),
        };
        let enc = n.encode();
        let dec = Node::decode(&enc).unwrap();
        assert_eq!(n, dec);
    }

    #[test]
    fn node_roundtrip_empty_children() {
        let n = Node {
            left: NodeChild::Empty { height: 7 },
            right: NodeChild::Empty { height: 42 },
        };
        let enc = n.encode();
        let dec = Node::decode(&enc).unwrap();
        assert_eq!(n, dec);
    }
}
