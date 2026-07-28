#![forbid(unsafe_code)]
//! GVSP receiver thread, packet reassembly, and resend logic.
//!
//! Deliberately has no dependency on `aravis-port-device`/its `net` module: receiving GVSP only needs a raw
//! [`std::net::UdpSocket`]; the only outbound traffic (`PACKET_RESEND_CMD`) goes out through the
//! small [`ResendRequester`] trait, which the umbrella crate wires to the device's GVCP
//! transaction.

mod config;
mod reassembly;
mod receiver;

pub use aravis_port_core::gvcp::PacketResend;
pub use config::StreamConfig;
pub use receiver::{spawn, FrameCallback, StreamHandle};

/// The only outbound hook this crate needs: requesting a packet resend over the control channel.
pub trait ResendRequester: Send {
    fn request_resend(&mut self, resend: PacketResend);
}
