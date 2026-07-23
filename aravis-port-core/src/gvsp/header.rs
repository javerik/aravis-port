use crate::error::{Error, Result};

/// Status carried in the first 2 bytes of every GVSP datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GvspStatus {
    Success,
    PacketResend,
    Error(u16),
}

impl GvspStatus {
    pub fn from_u16(v: u16) -> Self {
        match v {
            0x0000 => GvspStatus::Success,
            0x0100 => GvspStatus::PacketResend,
            other => GvspStatus::Error(other),
        }
    }

    pub fn to_u16(self) -> u16 {
        match self {
            GvspStatus::Success => 0x0000,
            GvspStatus::PacketResend => 0x0100,
            GvspStatus::Error(v) => v,
        }
    }

    pub fn is_error(self) -> bool {
        matches!(self, GvspStatus::Error(_))
    }
}

/// The kind of a GVSP packet, encoded in bits `[30:24]` of `packet_infos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Unknown,
    Leader,
    Trailer,
    Payload,
    AllIn,
    H264,
    Multizone,
    Multipart,
    GenDc,
    Other(u8),
}

impl ContentType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => ContentType::Unknown,
            1 => ContentType::Leader,
            2 => ContentType::Trailer,
            3 => ContentType::Payload,
            4 => ContentType::AllIn,
            5 => ContentType::H264,
            6 => ContentType::Multizone,
            7 => ContentType::Multipart,
            8 => ContentType::GenDc,
            other => ContentType::Other(other),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            ContentType::Unknown => 0,
            ContentType::Leader => 1,
            ContentType::Trailer => 2,
            ContentType::Payload => 3,
            ContentType::AllIn => 4,
            ContentType::H264 => 5,
            ContentType::Multizone => 6,
            ContentType::Multipart => 7,
            ContentType::GenDc => 8,
            ContentType::Other(v) => v,
        }
    }
}

const EXTENDED_ID_MODE_MASK: u32 = 0x8000_0000;
const PACKET_ID_MASK: u32 = 0x00ff_ffff;
const CONTENT_TYPE_MASK: u32 = 0x7f00_0000;
const CONTENT_TYPE_SHIFT: u32 = 24;

/// A parsed GVSP packet header (standard 16-bit or extended 64-bit frame id variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GvspHeader {
    Standard {
        frame_id: u16,
        content_type: ContentType,
        /// 24-bit packet id.
        packet_id: u32,
    },
    Extended {
        flags: u16,
        content_type: ContentType,
        frame_id: u64,
        packet_id: u32,
    },
}

impl GvspHeader {
    pub fn frame_id(&self) -> u64 {
        match self {
            GvspHeader::Standard { frame_id, .. } => *frame_id as u64,
            GvspHeader::Extended { frame_id, .. } => *frame_id,
        }
    }

    pub fn packet_id(&self) -> u32 {
        match self {
            GvspHeader::Standard { packet_id, .. } => *packet_id,
            GvspHeader::Extended { packet_id, .. } => *packet_id,
        }
    }

    pub fn content_type(&self) -> ContentType {
        match self {
            GvspHeader::Standard { content_type, .. } => *content_type,
            GvspHeader::Extended { content_type, .. } => *content_type,
        }
    }

    pub fn is_extended(&self) -> bool {
        matches!(self, GvspHeader::Extended { .. })
    }

    /// Parse the leading status + header of a raw GVSP datagram. Returns the status, the parsed
    /// header, and the remaining payload bytes.
    pub fn parse(bytes: &[u8]) -> Result<(GvspStatus, Self, &[u8])> {
        if bytes.len() < 2 {
            return Err(Error::Truncated { need: 2, got: bytes.len() });
        }
        let status = GvspStatus::from_u16(u16::from_be_bytes([bytes[0], bytes[1]]));
        let rest = &bytes[2..];
        if rest.len() < 6 {
            return Err(Error::Truncated { need: 8, got: bytes.len() });
        }
        let packet_infos_probe = u32::from_be_bytes([rest[2], rest[3], rest[4], rest[5]]);
        if packet_infos_probe & EXTENDED_ID_MODE_MASK != 0 {
            if rest.len() < 18 {
                return Err(Error::Truncated { need: 20, got: bytes.len() });
            }
            let flags = u16::from_be_bytes([rest[0], rest[1]]);
            let packet_infos = u32::from_be_bytes([rest[2], rest[3], rest[4], rest[5]]);
            let content_type = ContentType::from_u8(((packet_infos & CONTENT_TYPE_MASK) >> CONTENT_TYPE_SHIFT) as u8);
            let frame_id = u64::from_be_bytes(rest[6..14].try_into().unwrap());
            let packet_id = u32::from_be_bytes(rest[14..18].try_into().unwrap());
            Ok((
                status,
                GvspHeader::Extended {
                    flags,
                    content_type,
                    frame_id,
                    packet_id,
                },
                &rest[18..],
            ))
        } else {
            let frame_id = u16::from_be_bytes([rest[0], rest[1]]);
            let packet_infos = packet_infos_probe;
            let content_type = ContentType::from_u8(((packet_infos & CONTENT_TYPE_MASK) >> CONTENT_TYPE_SHIFT) as u8);
            let packet_id = packet_infos & PACKET_ID_MASK;
            Ok((
                status,
                GvspHeader::Standard {
                    frame_id,
                    content_type,
                    packet_id,
                },
                &rest[6..],
            ))
        }
    }

    /// Serialize status + header into a `Vec<u8>` (used by the fake camera / tests).
    pub fn to_bytes(&self, status: GvspStatus) -> Vec<u8> {
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(&status.to_u16().to_be_bytes());
        match *self {
            GvspHeader::Standard {
                frame_id,
                content_type,
                packet_id,
            } => {
                buf.extend_from_slice(&frame_id.to_be_bytes());
                let packet_infos = ((content_type.to_u8() as u32) << CONTENT_TYPE_SHIFT) | (packet_id & PACKET_ID_MASK);
                buf.extend_from_slice(&packet_infos.to_be_bytes());
            }
            GvspHeader::Extended {
                flags,
                content_type,
                frame_id,
                packet_id,
            } => {
                buf.extend_from_slice(&flags.to_be_bytes());
                let packet_infos = EXTENDED_ID_MODE_MASK | ((content_type.to_u8() as u32) << CONTENT_TYPE_SHIFT);
                buf.extend_from_slice(&packet_infos.to_be_bytes());
                buf.extend_from_slice(&frame_id.to_be_bytes());
                buf.extend_from_slice(&packet_id.to_be_bytes());
            }
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_header_round_trips() {
        let header = GvspHeader::Standard {
            frame_id: 65400,
            content_type: ContentType::Payload,
            packet_id: 12,
        };
        let bytes = header.to_bytes(GvspStatus::Success);
        let (status, parsed, rest) = GvspHeader::parse(&bytes).unwrap();
        assert_eq!(status, GvspStatus::Success);
        assert_eq!(parsed, header);
        assert!(rest.is_empty());
    }

    #[test]
    fn extended_header_round_trips() {
        let header = GvspHeader::Extended {
            flags: 0,
            content_type: ContentType::Leader,
            frame_id: 0x1_0000_0002,
            packet_id: 0xdead_beef,
        };
        let bytes = header.to_bytes(GvspStatus::Success);
        let (_, parsed, _) = GvspHeader::parse(&bytes).unwrap();
        assert_eq!(parsed, header);
        assert!(parsed.is_extended());
        assert_eq!(parsed.frame_id(), 0x1_0000_0002);
    }

    #[test]
    fn error_status_round_trips() {
        let header = GvspHeader::Standard {
            frame_id: 1,
            content_type: ContentType::Trailer,
            packet_id: 3,
        };
        let bytes = header.to_bytes(GvspStatus::Error(0x800c));
        let (status, _, _) = GvspHeader::parse(&bytes).unwrap();
        assert_eq!(status, GvspStatus::Error(0x800c));
        assert!(status.is_error());
    }

    #[test]
    fn truncated_input_errors() {
        assert!(GvspHeader::parse(&[0, 0]).is_err());
        assert!(GvspHeader::parse(&[]).is_err());
    }
}
