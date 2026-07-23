//! GVCP (GigE Vision Control Protocol) packet types.

mod header;
mod messages;

pub use header::{next_packet_id, Command, GvcpHeader, PacketFlags, PacketType, HEADER_LEN};
pub use messages::{
    DiscoveryAck, GvcpPayload, PacketResend, PendingAck, ReadMemoryAck, ReadMemoryCmd,
    ReadRegisterAck, ReadRegisterCmd, WriteMemoryAck, WriteMemoryCmd, WriteRegisterAck,
    WriteRegisterCmd, GVCP_DATA_SIZE_MAX,
};

/// Well-known UDP port for GVCP traffic.
pub const PORT: u16 = 3956;
