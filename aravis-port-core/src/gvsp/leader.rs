use crate::error::{Error, Result};

/// Low 14 bits of a leader's `payload_type` field (bit 14 separately signals chunk data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Image,
    RawData,
    File,
    ChunkData,
    ExtendedChunkData,
    Jpeg,
    Jpeg2000,
    H264,
    MultizoneImage,
    Multipart,
    GenDcContainer,
    GenDcComponentData,
    Unknown(u16),
}

impl PayloadKind {
    pub fn from_u16(v: u16) -> Self {
        match v & 0x3fff {
            1 => PayloadKind::Image,
            2 => PayloadKind::RawData,
            3 => PayloadKind::File,
            4 => PayloadKind::ChunkData,
            5 => PayloadKind::ExtendedChunkData,
            6 => PayloadKind::Jpeg,
            7 => PayloadKind::Jpeg2000,
            8 => PayloadKind::H264,
            9 => PayloadKind::MultizoneImage,
            0xa => PayloadKind::Multipart,
            0xb => PayloadKind::GenDcContainer,
            0xc => PayloadKind::GenDcComponentData,
            other => PayloadKind::Unknown(other),
        }
    }

    pub fn to_u16(self) -> u16 {
        match self {
            PayloadKind::Image => 1,
            PayloadKind::RawData => 2,
            PayloadKind::File => 3,
            PayloadKind::ChunkData => 4,
            PayloadKind::ExtendedChunkData => 5,
            PayloadKind::Jpeg => 6,
            PayloadKind::Jpeg2000 => 7,
            PayloadKind::H264 => 8,
            PayloadKind::MultizoneImage => 9,
            PayloadKind::Multipart => 0xa,
            PayloadKind::GenDcContainer => 0xb,
            PayloadKind::GenDcComponentData => 0xc,
            PayloadKind::Unknown(v) => v,
        }
    }
}

const HAS_CHUNKS_BIT: u16 = 0x4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfos {
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub x_padding: u16,
    pub y_padding: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaderPayload {
    pub flags: u16,
    /// Raw payload_type field (kind in low 14 bits, has_chunks in bit 14).
    pub payload_type: u16,
    pub timestamp: u64,
    /// The image layout: from the image leader for `Image` payloads, and from the first part
    /// descriptor for `Multipart` payloads (as Aravis's buffer image accessors default to part 0).
    pub image: Option<ImageInfos>,
}

impl LeaderPayload {
    pub fn kind(&self) -> PayloadKind {
        PayloadKind::from_u16(self.payload_type)
    }

    pub fn has_chunks(&self) -> bool {
        self.payload_type & HAS_CHUNKS_BIT != 0
            || matches!(self.kind(), PayloadKind::ChunkData | PayloadKind::ExtendedChunkData)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + 28);
        buf.extend_from_slice(&self.flags.to_be_bytes());
        buf.extend_from_slice(&self.payload_type.to_be_bytes());
        buf.extend_from_slice(&((self.timestamp >> 32) as u32).to_be_bytes());
        buf.extend_from_slice(&(self.timestamp as u32).to_be_bytes());
        if let Some(img) = self.image {
            buf.extend_from_slice(&img.pixel_format.to_be_bytes());
            buf.extend_from_slice(&img.width.to_be_bytes());
            buf.extend_from_slice(&img.height.to_be_bytes());
            buf.extend_from_slice(&img.x_offset.to_be_bytes());
            buf.extend_from_slice(&img.y_offset.to_be_bytes());
            buf.extend_from_slice(&img.x_padding.to_be_bytes());
            buf.extend_from_slice(&img.y_padding.to_be_bytes());
        }
        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 12 {
            return Err(Error::Truncated { need: 12, got: bytes.len() });
        }
        let flags = u16::from_be_bytes([bytes[0], bytes[1]]);
        let payload_type = u16::from_be_bytes([bytes[2], bytes[3]]);
        let ts_high = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let ts_low = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        let timestamp = ((ts_high as u64) << 32) | ts_low as u64;
        let kind = PayloadKind::from_u16(payload_type);
        let image = if matches!(kind, PayloadKind::Image) && bytes.len() >= 12 + 24 {
            let b = &bytes[12..];
            Some(ImageInfos {
                pixel_format: u32::from_be_bytes(b[0..4].try_into().unwrap()),
                width: u32::from_be_bytes(b[4..8].try_into().unwrap()),
                height: u32::from_be_bytes(b[8..12].try_into().unwrap()),
                x_offset: u32::from_be_bytes(b[12..16].try_into().unwrap()),
                y_offset: u32::from_be_bytes(b[16..20].try_into().unwrap()),
                x_padding: u16::from_be_bytes(b[20..22].try_into().unwrap()),
                y_padding: u16::from_be_bytes(b[22..24].try_into().unwrap()),
            })
        } else if matches!(kind, PayloadKind::Multipart) && bytes.len() >= 12 + super::PART_INFOS_LEN {
            Some(super::PartInfos::decode(&bytes[12..])?.image)
        } else {
            None
        };
        Ok(Self {
            flags,
            payload_type,
            timestamp,
            image,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_leader_round_trips() {
        let leader = LeaderPayload {
            flags: 0,
            payload_type: PayloadKind::Image.to_u16(),
            timestamp: 0x1_2345_6789,
            image: Some(ImageInfos {
                pixel_format: 0x0101_0001, // Mono8
                width: 64,
                height: 1088,
                x_offset: 0,
                y_offset: 0,
                x_padding: 0,
                y_padding: 0,
            }),
        };
        let bytes = leader.encode();
        let decoded = LeaderPayload::decode(&bytes).unwrap();
        assert_eq!(decoded, leader);
        assert_eq!(decoded.kind(), PayloadKind::Image);
        assert!(!decoded.has_chunks());
    }

    #[test]
    fn non_image_leader_has_no_image_infos() {
        let leader = LeaderPayload {
            flags: 0,
            payload_type: PayloadKind::ChunkData.to_u16(),
            timestamp: 5,
            image: None,
        };
        let decoded = LeaderPayload::decode(&leader.encode()).unwrap();
        assert!(decoded.image.is_none());
        assert!(decoded.has_chunks());
    }

    #[test]
    fn truncated_leader_errors() {
        assert!(LeaderPayload::decode(&[0u8; 4]).is_err());
    }
}
