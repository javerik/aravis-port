use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailerPayload {
    pub payload_type: u32,
    /// Image height for image payloads.
    pub data0: u32,
}

impl TrailerPayload {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8);
        buf.extend_from_slice(&self.payload_type.to_be_bytes());
        buf.extend_from_slice(&self.data0.to_be_bytes());
        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(Error::Truncated { need: 8, got: bytes.len() });
        }
        Ok(Self {
            payload_type: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            data0: u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_round_trips() {
        let trailer = TrailerPayload {
            payload_type: 1,
            data0: 1088,
        };
        assert_eq!(TrailerPayload::decode(&trailer.encode()).unwrap(), trailer);
    }

    #[test]
    fn truncated_trailer_errors() {
        assert!(TrailerPayload::decode(&[0u8; 4]).is_err());
    }
}
