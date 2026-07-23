#![forbid(unsafe_code)]
//! Protocol types shared across the `aravis-port` workspace: GVCP/GVSP packet structs, GVBS
//! bootstrap register offsets, `MacAddress`, and the crate-wide `Error`/`Result` types.
//!
//! ```
//! use aravis_port_core::gvcp::{GvcpHeader, PacketType, Command};
//!
//! let header = GvcpHeader { packet_type: PacketType::Cmd, raw_flags: 0, command: Command::ReadRegisterCmd, size: 4, id: 1 };
//! let bytes = header.to_bytes();
//! assert_eq!(GvcpHeader::from_bytes(&bytes).unwrap(), header);
//! ```

pub mod bootstrap;
pub mod error;
pub mod gvcp;
pub mod gvsp;
pub mod mac;

pub use error::{Error, Result};
pub use mac::MacAddress;
