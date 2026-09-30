//! GVSP (GigE Vision Streaming Protocol) packet types.

mod header;
mod leader;
mod multipart;
mod trailer;

pub use header::{ContentType, GvspHeader, GvspStatus};
pub use leader::{ImageInfos, LeaderPayload, PayloadKind};
pub use multipart::{MultipartBlock, PartInfos, MULTIPART_BLOCK_HEADER_LEN, PART_INFOS_LEN};
pub use trailer::TrailerPayload;

/// Minimum/maximum GVSP payload sizes (excluding the 2-byte status prefix).
pub const MINIMUM_PACKET_SIZE: usize = 46;
pub const MAXIMUM_PACKET_SIZE: usize = 65536 - 20 - 8;
/// Common default packet size matching a standard Ethernet MTU.
pub const DEFAULT_PACKET_SIZE: u16 = 1500;
