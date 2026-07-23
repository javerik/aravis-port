use crate::gvcp::Command;

/// Errors shared by every layer of the aravis-port stack.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("truncated packet: need {need} bytes, got {got}")]
    Truncated { need: usize, got: usize },

    #[error("gvcp error ack: code=0x{0:02x}")]
    GvcpError(u8),

    #[error("timeout waiting for {command:?} ack (id={id})")]
    Timeout { command: Command, id: u16 },

    #[error("unexpected ack: expected {expected:?}/{expected_id}, got {got:?}/{got_id}")]
    UnexpectedAck {
        expected: Command,
        expected_id: u16,
        got: Command,
        got_id: u16,
    },

    #[error("genicam: {0}")]
    GenIcam(String),

    #[error("feature '{0}' not found")]
    FeatureNotFound(String),

    #[error("feature '{name}': expected {expected}, found {found}")]
    FeatureTypeMismatch {
        name: String,
        expected: &'static str,
        found: &'static str,
    },

    #[error("device does not hold control privilege")]
    NotController,

    #[error("device disconnected")]
    Disconnected,
}

pub type Result<T> = std::result::Result<T, Error>;
