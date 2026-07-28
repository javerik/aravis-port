use std::sync::mpsc;

use super::buffer::Buffer;

/// The user-facing half of a buffer pool: push empty (or reused) buffers in, pop completed ones
/// out. Mirrors Aravis's two-`GAsyncQueue` model without needing a literal ring buffer.
pub struct BufferPoolHandle {
    pub to_stream: mpsc::Sender<Buffer>,
    pub from_stream: mpsc::Receiver<Buffer>,
}

impl BufferPoolHandle {
    pub fn push_buffer(&self, buffer: Buffer) {
        // The stream thread may have exited already (e.g. after `stop_acquisition`); dropping
        // the buffer is the correct behavior in that case, not an error.
        let _ = self.to_stream.send(buffer);
    }

    pub fn try_pop_buffer(&self) -> Option<Buffer> {
        self.from_stream.try_recv().ok()
    }

    pub fn pop_buffer(&self) -> Option<Buffer> {
        self.from_stream.recv().ok()
    }

    pub fn timeout_pop_buffer(&self, timeout: std::time::Duration) -> Option<Buffer> {
        self.from_stream.recv_timeout(timeout).ok()
    }
}

/// The stream-thread-facing half: pop empty buffers to fill, push completed ones back.
pub struct BufferPoolStreamSide {
    pub from_user: mpsc::Receiver<Buffer>,
    pub to_user: mpsc::Sender<Buffer>,
}

impl BufferPoolStreamSide {
    pub fn pop_input_buffer(&self) -> Option<Buffer> {
        self.from_user.try_recv().ok()
    }

    pub fn push_output_buffer(&self, buffer: Buffer) {
        let _ = self.to_user.send(buffer);
    }
}

/// Create a linked pool with `n` pre-allocated buffers of `payload_capacity` bytes, already
/// queued on the user side ready to be handed to a stream.
pub fn new_buffer_pool(n: usize, payload_capacity: usize) -> (BufferPoolHandle, BufferPoolStreamSide) {
    let (to_stream_tx, to_stream_rx) = mpsc::channel();
    let (to_user_tx, to_user_rx) = mpsc::channel();

    for _ in 0..n {
        let _ = to_stream_tx.send(Buffer::new(payload_capacity));
    }

    (
        BufferPoolHandle {
            to_stream: to_stream_tx,
            from_stream: to_user_rx,
        },
        BufferPoolStreamSide {
            from_user: to_stream_rx,
            to_user: to_user_tx,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preallocated_buffers_are_available_to_the_stream_side() {
        let (_user, stream) = new_buffer_pool(3, 64);
        let mut count = 0;
        while stream.pop_input_buffer().is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn round_trip_through_both_sides() {
        let (user, stream) = new_buffer_pool(1, 16);
        let mut buf = stream.pop_input_buffer().unwrap();
        buf.frame_id = 42;
        stream.push_output_buffer(buf);
        let received = user.pop_buffer().unwrap();
        assert_eq!(received.frame_id, 42);
    }
}
