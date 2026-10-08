use crate::error::{Error, Result};

/// The kind of a GVCP packet, carried in byte 0 of every header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Ack,
    Cmd,
    Error,
    UnknownError,
}

impl PacketType {
    pub const fn to_byte(self) -> u8 {
        match self {
            PacketType::Ack => 0x00,
            PacketType::Cmd => 0x42,
            PacketType::Error => 0x80,
            PacketType::UnknownError => 0x8f,
        }
    }

    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(PacketType::Ack),
            0x42 => Some(PacketType::Cmd),
            0x80 => Some(PacketType::Error),
            0x8f => Some(PacketType::UnknownError),
            _ => None,
        }
    }
}

bitflags::bitflags! {
    /// Flags carried in byte 1 of a command header. On an `Error` packet, that same byte
    /// instead holds the raw `ArvGvcpError` code — see [`super::header::GvcpHeader::error_code`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PacketFlags: u8 {
        const ACK_REQUIRED = 0x01;
        const EXTENDED_IDS = 0x10;
        /// Discovery-specific reuse of the same bit position as `EXTENDED_IDS`.
        const ALLOW_BROADCAST_ACK = 0x10;
    }
}

/// GVCP command codes. `Unknown` keeps `from_code` infallible for forward compatibility with
/// commands this crate doesn't otherwise model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    DiscoveryCmd,
    DiscoveryAck,
    ByeCmd,
    ByeAck,
    PacketResendCmd,
    PacketResendAck,
    ReadRegisterCmd,
    ReadRegisterAck,
    WriteRegisterCmd,
    WriteRegisterAck,
    ReadMemoryCmd,
    ReadMemoryAck,
    WriteMemoryCmd,
    WriteMemoryAck,
    PendingAck,
    Unknown(u16),
}

impl Command {
    pub const fn code(self) -> u16 {
        match self {
            Command::DiscoveryCmd => 0x0002,
            Command::DiscoveryAck => 0x0003,
            Command::ByeCmd => 0x0004,
            Command::ByeAck => 0x0005,
            Command::PacketResendCmd => 0x0040,
            Command::PacketResendAck => 0x0041,
            Command::ReadRegisterCmd => 0x0080,
            Command::ReadRegisterAck => 0x0081,
            Command::WriteRegisterCmd => 0x0082,
            Command::WriteRegisterAck => 0x0083,
            Command::ReadMemoryCmd => 0x0084,
            Command::ReadMemoryAck => 0x0085,
            Command::WriteMemoryCmd => 0x0086,
            Command::WriteMemoryAck => 0x0087,
            Command::PendingAck => 0x0089,
            Command::Unknown(c) => c,
        }
    }

    pub const fn from_code(code: u16) -> Self {
        match code {
            0x0002 => Command::DiscoveryCmd,
            0x0003 => Command::DiscoveryAck,
            0x0004 => Command::ByeCmd,
            0x0005 => Command::ByeAck,
            0x0040 => Command::PacketResendCmd,
            0x0041 => Command::PacketResendAck,
            0x0080 => Command::ReadRegisterCmd,
            0x0081 => Command::ReadRegisterAck,
            0x0082 => Command::WriteRegisterCmd,
            0x0083 => Command::WriteRegisterAck,
            0x0084 => Command::ReadMemoryCmd,
            0x0085 => Command::ReadMemoryAck,
            0x0086 => Command::WriteMemoryCmd,
            0x0087 => Command::WriteMemoryAck,
            0x0089 => Command::PendingAck,
            other => Command::Unknown(other),
        }
    }
}

/// The fixed 8-byte GVCP header, always big-endian on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvcpHeader {
    pub packet_type: PacketType,
    /// Raw byte 1: command flags for CMD/ACK packets, or the `ArvGvcpError` code for ERROR
    /// packets. Kept raw (rather than a `PacketFlags` bitflags value) because its meaning is
    /// type-dependent.
    pub raw_flags: u8,
    pub command: Command,
    /// Payload size in bytes, following the header.
    pub size: u16,
    /// Transaction id. `0` is reserved/invalid; ids wrap `0xffff -> 1`.
    pub id: u16,
}

pub const HEADER_LEN: usize = 8;

impl GvcpHeader {
    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0] = self.packet_type.to_byte();
        buf[1] = self.raw_flags;
        buf[2..4].copy_from_slice(&self.command.code().to_be_bytes());
        buf[4..6].copy_from_slice(&self.size.to_be_bytes());
        buf[6..8].copy_from_slice(&self.id.to_be_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::Truncated {
                need: HEADER_LEN,
                got: bytes.len(),
            });
        }
        let packet_type = PacketType::from_byte(bytes[0]).unwrap_or(PacketType::UnknownError);
        let command = Command::from_code(u16::from_be_bytes([bytes[2], bytes[3]]));
        let size = u16::from_be_bytes([bytes[4], bytes[5]]);
        let id = u16::from_be_bytes([bytes[6], bytes[7]]);
        Ok(Self {
            packet_type,
            raw_flags: bytes[1],
            command,
            size,
            id,
        })
    }

    /// If this is an `Error`/`UnknownError` packet, the `ArvGvcpError` code carried in `raw_flags`.
    pub fn error_code(&self) -> Option<u8> {
        matches!(
            self.packet_type,
            PacketType::Error | PacketType::UnknownError
        )
        .then_some(self.raw_flags)
    }
}

/// Advance a GVCP transaction id, skipping the reserved value `0` and wrapping `0xffff -> 1`.
pub const fn next_packet_id(current: u16) -> u16 {
    if current == 0xffff || current == 0 {
        1
    } else {
        current + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let h = GvcpHeader {
            packet_type: PacketType::Cmd,
            raw_flags: PacketFlags::ACK_REQUIRED.bits(),
            command: Command::ReadRegisterCmd,
            size: 4,
            id: 42,
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes, [0x42, 0x01, 0x00, 0x80, 0x00, 0x04, 0x00, 0x2a]);
        assert_eq!(GvcpHeader::from_bytes(&bytes).unwrap(), h);
    }

    #[test]
    fn error_packet_exposes_code() {
        let h = GvcpHeader {
            packet_type: PacketType::Error,
            raw_flags: 0x17,
            command: Command::ReadRegisterCmd,
            size: 0,
            id: 1,
        };
        assert_eq!(h.error_code(), Some(0x17));
    }

    #[test]
    fn truncated_bytes_error_instead_of_panicking() {
        assert!(matches!(
            GvcpHeader::from_bytes(&[0u8; 4]),
            Err(Error::Truncated { need: 8, got: 4 })
        ));
    }

    #[test]
    fn id_wraps_skipping_zero() {
        assert_eq!(next_packet_id(0xfffe), 0xffff);
        assert_eq!(next_packet_id(0xffff), 1);
    }
}
