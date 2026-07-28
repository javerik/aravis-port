/// Outcome of a single frame's acquisition attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferStatus {
    Success,
    Cleared,
    Timeout,
    MissingPackets,
    WrongPacketId,
    SizeMismatch,
    Filling,
    Aborted,
    PayloadNotSupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadType {
    Image,
    RawData,
    File,
    ChunkData,
    ExtendedChunkData,
    Jpeg,
    Jpeg2000,
    H264,
    MultizoneImage,
    Multipart,
    GenDcContainer,
    GenDcComponentData,
    NoData,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
}

/// A single acquired (or in-progress) frame. Reused across acquisitions: push it back into a
/// [`crate::memory::pool`] once consumed rather than allocating a new one per frame.
#[derive(Debug, Clone)]
pub struct Buffer {
    pub frame_id: u64,
    pub status: BufferStatus,
    pub payload_type: PayloadType,
    pub timestamp_ns: u64,
    pub image: Option<ImageInfo>,
    data: Vec<u8>,
}

impl Buffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            frame_id: 0,
            status: BufferStatus::Cleared,
            payload_type: PayloadType::Unknown,
            timestamp_ns: 0,
            image: None,
            data: vec![0u8; capacity],
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.data
    }

    /// Reset bookkeeping fields for reuse, keeping the allocated `data` capacity.
    pub fn reset_for_reuse(&mut self) {
        self.frame_id = 0;
        self.status = BufferStatus::Cleared;
        self.payload_type = PayloadType::Unknown;
        self.timestamp_ns = 0;
        self.image = None;
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_for_reuse_clears_bookkeeping_but_keeps_capacity() {
        let mut buf = Buffer::new(1024);
        buf.data_mut().extend_from_slice(&[1, 2, 3]);
        buf.frame_id = 7;
        buf.status = BufferStatus::Success;
        buf.reset_for_reuse();
        assert_eq!(buf.frame_id, 0);
        assert_eq!(buf.status, BufferStatus::Cleared);
        assert!(buf.data().is_empty());
        assert!(buf.data.capacity() >= 1024);
    }
}
