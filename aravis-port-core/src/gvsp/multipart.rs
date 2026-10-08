//! GVSP multi-part payload (GigE Vision 2.x): the per-part descriptors carried by a multi-part
//! leader, and the prefix of every multi-part data packet. Confirmed necessary against the live
//! C6-2040-GigE, which streams its intensity image as a one-part multi-part payload.

use crate::error::{Error, Result};

use super::leader::ImageInfos;

/// Size of one part descriptor in a multi-part leader (`ArvGvspPartInfos` in Aravis).
pub const PART_INFOS_LEN: usize = 48;
/// Size of the prefix in front of the data of every multi-part data packet.
pub const MULTIPART_BLOCK_HEADER_LEN: usize = 8;

/// One part descriptor of a multi-part leader. Fields follow Aravis's `ArvGvspPartInfos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartInfos {
    pub data_type: u16,
    /// Part length in bytes (48-bit on the wire).
    pub length: u64,
    pub source_id: u8,
    pub additional_zones: u8,
    pub zone_directions: u32,
    pub data_purpose_id: u16,
    pub region_id: u16,
    pub image: ImageInfos,
}

impl PartInfos {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < PART_INFOS_LEN {
            return Err(Error::Truncated {
                need: PART_INFOS_LEN,
                got: bytes.len(),
            });
        }
        let u16_at = |i: usize| u16::from_be_bytes([bytes[i], bytes[i + 1]]);
        let u32_at = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
        Ok(Self {
            data_type: u16_at(0),
            length: ((u16_at(2) as u64) << 32) | u32_at(4) as u64,
            // bytes 12..14 are reserved
            source_id: bytes[14],
            additional_zones: bytes[15],
            zone_directions: u32_at(16),
            data_purpose_id: u16_at(20),
            region_id: u16_at(22),
            image: ImageInfos {
                pixel_format: u32_at(8),
                width: u32_at(24),
                height: u32_at(28),
                x_offset: u32_at(32),
                y_offset: u32_at(36),
                x_padding: u16_at(40),
                y_padding: u16_at(42),
            },
            // bytes 44..48 are reserved
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(PART_INFOS_LEN);
        buf.extend_from_slice(&self.data_type.to_be_bytes());
        buf.extend_from_slice(&((self.length >> 32) as u16).to_be_bytes());
        buf.extend_from_slice(&(self.length as u32).to_be_bytes());
        buf.extend_from_slice(&self.image.pixel_format.to_be_bytes());
        buf.extend_from_slice(&[0, 0, self.source_id, self.additional_zones]);
        buf.extend_from_slice(&self.zone_directions.to_be_bytes());
        buf.extend_from_slice(&self.data_purpose_id.to_be_bytes());
        buf.extend_from_slice(&self.region_id.to_be_bytes());
        buf.extend_from_slice(&self.image.width.to_be_bytes());
        buf.extend_from_slice(&self.image.height.to_be_bytes());
        buf.extend_from_slice(&self.image.x_offset.to_be_bytes());
        buf.extend_from_slice(&self.image.y_offset.to_be_bytes());
        buf.extend_from_slice(&self.image.x_padding.to_be_bytes());
        buf.extend_from_slice(&self.image.y_padding.to_be_bytes());
        buf.extend_from_slice(&[0; 4]);
        buf
    }
}

/// The prefix of a multi-part data packet (`ArvGvspMultipart` in Aravis). Unlike a generic
/// payload packet, whose position follows from its packet id, a multi-part block carries its
/// byte offset explicitly. Aravis treats that offset as relative to the start of the whole
/// payload buffer, and so does this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultipartBlock {
    pub part_id: u8,
    pub zone_info: u8,
    /// Byte offset of this block's data (48-bit on the wire).
    pub offset: u64,
}

impl MultipartBlock {
    /// Split a multi-part data packet's payload into its prefix and its data bytes.
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8])> {
        if bytes.len() < MULTIPART_BLOCK_HEADER_LEN {
            return Err(Error::Truncated {
                need: MULTIPART_BLOCK_HEADER_LEN,
                got: bytes.len(),
            });
        }
        let offset_high = u16::from_be_bytes([bytes[2], bytes[3]]) as u64;
        let offset_low = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as u64;
        Ok((
            Self {
                part_id: bytes[0],
                zone_info: bytes[1],
                offset: (offset_high << 32) | offset_low,
            },
            &bytes[MULTIPART_BLOCK_HEADER_LEN..],
        ))
    }

    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MULTIPART_BLOCK_HEADER_LEN + data.len());
        buf.extend_from_slice(&[self.part_id, self.zone_info]);
        buf.extend_from_slice(&((self.offset >> 32) as u16).to_be_bytes());
        buf.extend_from_slice(&(self.offset as u32).to_be_bytes());
        buf.extend_from_slice(data);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_infos_decode_the_live_c6_leader_descriptor() {
        // The single part descriptor of a real C6-2040-GigE leader: a 2D Mono8 2048x1088 image
        // of 0x220080 bytes.
        let bytes = [
            0x00, 0x01, 0x00, 0x00, 0x00, 0x22, 0x00, 0x80, 0x01, 0x08, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00,
            0x00, 0x00, 0x04, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let part = PartInfos::decode(&bytes).unwrap();
        assert_eq!(part.data_type, 1);
        assert_eq!(part.length, 0x22_0080);
        assert_eq!(part.image.pixel_format, 0x0108_0001);
        assert_eq!(part.data_purpose_id, 1);
        assert_eq!((part.image.width, part.image.height), (2048, 1088));
        assert_eq!(PartInfos::decode(&part.encode()).unwrap(), part);
    }

    #[test]
    fn multipart_block_round_trips_with_a_48_bit_offset() {
        let block = MultipartBlock {
            part_id: 2,
            zone_info: 0x40,
            offset: 0x0001_0021_fb40,
        };
        let bytes = block.encode(b"data");
        let (decoded, data) = MultipartBlock::decode(&bytes).unwrap();
        assert_eq!(decoded, block);
        assert_eq!(data, b"data");
    }

    #[test]
    fn truncated_multipart_input_is_an_error_not_a_panic() {
        assert!(MultipartBlock::decode(&[0; MULTIPART_BLOCK_HEADER_LEN - 1]).is_err());
        assert!(PartInfos::decode(&[0; PART_INFOS_LEN - 1]).is_err());
    }
}
