//! UDP-based discovery and GVCP request/response handling.
//!
//! `std::net::UdpSocket` has no safe way to enumerate local network interfaces, so unlike
//! Aravis's automatic per-NIC discovery, [`discover`] requires callers to supply explicit bind
//! addresses via [`DiscoveryOptions`].
//!
//! ## A note on throughput (line-rate streaming)
//!
//! `std::net::UdpSocket` also has no safe way to set `SO_RCVBUF` (the kernel receive-buffer
//! size) — that would require a dependency like `socket2`, which is outside this project's
//! closed dependency list. At sustained high frame rates or large images, an undersized OS
//! default receive buffer can cause the kernel to drop GVSP datagrams before this crate ever
//! sees them (invisible to `aravis-port-stream`'s resend logic, since a resend can't recover a
//! packet the OS silently discarded). Live testing against a real GigE Vision camera (25 fps,
//! ~70 KB frames) showed a >98% first-attempt packet success rate with this system's default
//! `net.core.rmem_max`/`rmem_default` (16 MiB) — if a deployment sees persistently high
//! `MissingPackets` rates despite resend, the fix is an external `sysctl` bump
//! (`net.core.rmem_max`, `net.core.rmem_default`), not a code change here.

mod discovery;
mod transaction;

pub use discovery::{discover, DiscoveredDevice, DiscoveryOptions};
pub use transaction::{GvcpTransaction, TransactionConfig};
