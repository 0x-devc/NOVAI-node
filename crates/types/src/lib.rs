#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tx {
    pub id: u64,
    pub payload: Vec<u8>,
}

impl Tx {
    pub fn new(id: u64, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            id,
            payload: payload.into(),
        }
    }
}
