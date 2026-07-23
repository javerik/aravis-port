use std::time::Duration;

/// Tuning knobs for GVSP reassembly and packet-resend behavior.
#[derive(Debug, Clone, Copy)]
pub struct StreamConfig {
    /// GVSP packet size, used to compute each payload packet's byte capacity.
    pub packet_size: u16,
    /// How long a packet may be missing before it's first flagged for resend.
    pub initial_packet_timeout: Duration,
    /// How long to wait after requesting a resend before trying again / giving up on it.
    pub packet_timeout: Duration,
    /// Maximum age of a frame (since its last received packet) before it's force-closed.
    pub frame_retention: Duration,
    /// Cap on the fraction of a frame's packets that may be resend-requested.
    pub packet_request_ratio: f32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            packet_size: 1500,
            initial_packet_timeout: Duration::from_micros(1_000),
            packet_timeout: Duration::from_micros(20_000),
            frame_retention: Duration::from_micros(100_000),
            packet_request_ratio: 0.25,
        }
    }
}
