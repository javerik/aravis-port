use std::net::Ipv4Addr;

use crate::bootstrap::{offset, BootstrapView};
use crate::error::{Error, Result};
use crate::gvcp::header::Command;
use crate::mac::MacAddress;

/// Maximum payload size for a single `READ_MEMORY`/`WRITE_MEMORY` transaction. Larger requests
/// must be chunked by the caller.
pub const GVCP_DATA_SIZE_MAX: usize = 512;

/// A GVCP payload that can be encoded for the wire and decoded from an ack/cmd body.
pub trait GvcpPayload: Sized {
    /// The command expected on the ack half of this payload's transaction.
    const ACK_COMMAND: Command;

    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Result<Self>;
}

fn require_len(bytes: &[u8], need: usize) -> Result<()> {
    if bytes.len() < need {
        Err(Error::Truncated {
            need,
            got: bytes.len(),
        })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRegisterCmd {
    pub address: u32,
}

impl GvcpPayload for ReadRegisterCmd {
    const ACK_COMMAND: Command = Command::ReadRegisterAck;
    fn encode(&self) -> Vec<u8> {
        self.address.to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            address: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRegisterAck {
    pub value: u32,
}

impl GvcpPayload for ReadRegisterAck {
    const ACK_COMMAND: Command = Command::ReadRegisterAck;
    fn encode(&self) -> Vec<u8> {
        self.value.to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            value: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteRegisterCmd {
    pub address: u32,
    pub value: u32,
}

impl GvcpPayload for WriteRegisterCmd {
    const ACK_COMMAND: Command = Command::WriteRegisterAck;
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8);
        buf.extend_from_slice(&self.address.to_be_bytes());
        buf.extend_from_slice(&self.value.to_be_bytes());
        buf
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 8)?;
        Ok(Self {
            address: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            value: u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteRegisterAck {
    pub data_index: u32,
}

impl GvcpPayload for WriteRegisterAck {
    const ACK_COMMAND: Command = Command::WriteRegisterAck;
    fn encode(&self) -> Vec<u8> {
        self.data_index.to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            data_index: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMemoryCmd {
    pub address: u32,
    /// Number of bytes requested; must be `<= GVCP_DATA_SIZE_MAX`.
    pub size: u16,
}

impl GvcpPayload for ReadMemoryCmd {
    const ACK_COMMAND: Command = Command::ReadMemoryAck;
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8);
        buf.extend_from_slice(&self.address.to_be_bytes());
        buf.extend_from_slice(&(self.size as u32).to_be_bytes());
        buf
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 8)?;
        let size = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        Ok(Self {
            address: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            size: (size & 0xffff) as u16,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMemoryAck {
    pub address: u32,
    pub data: Vec<u8>,
}

impl GvcpPayload for ReadMemoryAck {
    const ACK_COMMAND: Command = Command::ReadMemoryAck;
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + self.data.len());
        buf.extend_from_slice(&self.address.to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            address: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            data: bytes[4..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteMemoryCmd {
    pub address: u32,
    pub data: Vec<u8>,
}

impl GvcpPayload for WriteMemoryCmd {
    const ACK_COMMAND: Command = Command::WriteMemoryAck;
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + self.data.len());
        buf.extend_from_slice(&self.address.to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            address: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
            data: bytes[4..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteMemoryAck {
    pub address: u32,
}

impl GvcpPayload for WriteMemoryAck {
    const ACK_COMMAND: Command = Command::WriteMemoryAck;
    fn encode(&self) -> Vec<u8> {
        self.address.to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            address: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingAck {
    pub timeout_ms: u32,
}

impl GvcpPayload for PendingAck {
    const ACK_COMMAND: Command = Command::PendingAck;
    fn encode(&self) -> Vec<u8> {
        self.timeout_ms.to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, 4)?;
        Ok(Self {
            timeout_ms: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        })
    }
}

/// A `PACKET_RESEND_CMD` request, standard (16-bit frame id) or extended (64-bit frame id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketResend {
    Standard {
        frame_id: u32,
        /// Low 24 bits significant.
        first_block: u32,
        last_block: u32,
    },
    Extended {
        frame_id: u64,
        first_block: u32,
        last_block: u32,
    },
}

impl PacketResend {
    pub const ACK_COMMAND: Command = Command::PacketResendCmd;

    pub fn is_extended(&self) -> bool {
        matches!(self, PacketResend::Extended { .. })
    }

    pub fn encode(&self) -> Vec<u8> {
        match *self {
            PacketResend::Standard {
                frame_id,
                first_block,
                last_block,
            } => {
                let mut buf = Vec::with_capacity(12);
                buf.extend_from_slice(&frame_id.to_be_bytes());
                buf.extend_from_slice(&(first_block & 0x00ff_ffff).to_be_bytes());
                buf.extend_from_slice(&(last_block & 0x00ff_ffff).to_be_bytes());
                buf
            }
            PacketResend::Extended {
                frame_id,
                first_block,
                last_block,
            } => {
                let mut buf = Vec::with_capacity(20);
                buf.extend_from_slice(&0u32.to_be_bytes());
                buf.extend_from_slice(&first_block.to_be_bytes());
                buf.extend_from_slice(&last_block.to_be_bytes());
                buf.extend_from_slice(&frame_id.to_be_bytes());
                buf
            }
        }
    }

    pub fn decode(bytes: &[u8], extended: bool) -> Result<Self> {
        if extended {
            require_len(bytes, 20)?;
            Ok(PacketResend::Extended {
                first_block: u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
                last_block: u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
                frame_id: u64::from_be_bytes(bytes[12..20].try_into().unwrap()),
            })
        } else {
            require_len(bytes, 12)?;
            Ok(PacketResend::Standard {
                frame_id: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
                first_block: u32::from_be_bytes(bytes[4..8].try_into().unwrap()) & 0x00ff_ffff,
                last_block: u32::from_be_bytes(bytes[8..12].try_into().unwrap()) & 0x00ff_ffff,
            })
        }
    }
}

/// The 248-byte bootstrap-register mirror carried in a `DISCOVERY_ACK` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryAck(pub [u8; offset::DISCOVERY_DATA_SIZE]);

impl DiscoveryAck {
    fn view(&self) -> BootstrapView<'_> {
        BootstrapView(&self.0)
    }

    pub fn manufacturer(&self) -> String {
        self.view().manufacturer()
    }

    pub fn model(&self) -> String {
        self.view().model()
    }

    pub fn version(&self) -> String {
        self.view().version()
    }

    pub fn serial(&self) -> String {
        self.view().serial()
    }

    pub fn mac(&self) -> MacAddress {
        self.view().mac_address()
    }

    pub fn current_ip(&self) -> Ipv4Addr {
        self.view().current_ip()
    }
}

impl GvcpPayload for DiscoveryAck {
    const ACK_COMMAND: Command = Command::DiscoveryAck;
    fn encode(&self) -> Vec<u8> {
        self.0.to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        require_len(bytes, offset::DISCOVERY_DATA_SIZE)?;
        let mut buf = [0u8; offset::DISCOVERY_DATA_SIZE];
        buf.copy_from_slice(&bytes[..offset::DISCOVERY_DATA_SIZE]);
        Ok(Self(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_register_round_trip() {
        let cmd = ReadRegisterCmd { address: 0xa00 };
        assert_eq!(ReadRegisterCmd::decode(&cmd.encode()).unwrap(), cmd);
        let ack = ReadRegisterAck { value: 3 };
        assert_eq!(ReadRegisterAck::decode(&ack.encode()).unwrap(), ack);
    }

    #[test]
    fn write_register_round_trip() {
        let cmd = WriteRegisterCmd {
            address: 0xa00,
            value: 1,
        };
        assert_eq!(WriteRegisterCmd::decode(&cmd.encode()).unwrap(), cmd);
        let ack = WriteRegisterAck { data_index: 0 };
        assert_eq!(WriteRegisterAck::decode(&ack.encode()).unwrap(), ack);
    }

    #[test]
    fn read_memory_round_trip() {
        let cmd = ReadMemoryCmd {
            address: 0x200,
            size: 512,
        };
        assert_eq!(ReadMemoryCmd::decode(&cmd.encode()).unwrap(), cmd);
        let ack = ReadMemoryAck {
            address: 0x200,
            data: vec![1, 2, 3, 4],
        };
        assert_eq!(ReadMemoryAck::decode(&ack.encode()).unwrap(), ack);
    }

    #[test]
    fn write_memory_round_trip() {
        let cmd = WriteMemoryCmd {
            address: 0x200,
            data: vec![9, 9, 9],
        };
        assert_eq!(WriteMemoryCmd::decode(&cmd.encode()).unwrap(), cmd);
        let ack = WriteMemoryAck { address: 0x200 };
        assert_eq!(WriteMemoryAck::decode(&ack.encode()).unwrap(), ack);
    }

    #[test]
    fn pending_ack_round_trip() {
        let ack = PendingAck { timeout_ms: 1000 };
        assert_eq!(PendingAck::decode(&ack.encode()).unwrap(), ack);
    }

    #[test]
    fn packet_resend_round_trip_standard_and_extended() {
        let std_resend = PacketResend::Standard {
            frame_id: 7,
            first_block: 3,
            last_block: 9,
        };
        let bytes = std_resend.encode();
        assert_eq!(bytes.len(), 12);
        assert_eq!(PacketResend::decode(&bytes, false).unwrap(), std_resend);

        let ext_resend = PacketResend::Extended {
            frame_id: 0x1_0000_0007,
            first_block: 3,
            last_block: 9,
        };
        let bytes = ext_resend.encode();
        assert_eq!(bytes.len(), 20);
        assert_eq!(PacketResend::decode(&bytes, true).unwrap(), ext_resend);
    }

    #[test]
    fn discovery_ack_exposes_bootstrap_fields() {
        let mut raw = [0u8; offset::DISCOVERY_DATA_SIZE];
        let model = b"C5-2040-GigE";
        raw[offset::MODEL_NAME as usize..offset::MODEL_NAME as usize + model.len()]
            .copy_from_slice(model);
        let ack = DiscoveryAck::decode(&raw).unwrap();
        assert_eq!(ack.model(), "C5-2040-GigE");
    }

    #[test]
    fn truncated_payloads_error_instead_of_panicking() {
        assert!(matches!(
            ReadRegisterAck::decode(&[0, 1]),
            Err(Error::Truncated { need: 4, got: 2 })
        ));
        assert!(matches!(
            PacketResend::decode(&[0u8; 4], false),
            Err(Error::Truncated { need: 12, got: 4 })
        ));
    }
}
