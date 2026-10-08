use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aravis_port_core::gvsp::GvspHeader;
use aravis_port_core::memory::{Buffer, BufferPoolStreamSide};

use crate::config::StreamConfig;
use crate::reassembly::Reassembler;
use crate::ResendRequester;

/// Invoked synchronously on the receiver thread for every completed frame, in addition to (not
/// instead of) the buffer becoming available through the pool's output channel.
pub type FrameCallback = Box<dyn FnMut(&Buffer) + Send>;

const RECEIVE_POLL_TIMEOUT: Duration = Duration::from_millis(5);
const MAX_DATAGRAM_SIZE: usize = 65536;

/// Handle to a running GVSP receiver thread; dropping it (or calling [`StreamHandle::stop`])
/// stops the thread.
pub struct StreamHandle {
    stop_tx: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl StreamHandle {
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Spawn a dedicated receiver thread reading GVSP datagrams from `socket`, reassembling frames,
/// and delivering them via the buffer pool's output channel (and, if supplied, `callback`).
pub fn spawn(
    socket: UdpSocket,
    cfg: StreamConfig,
    pool: BufferPoolStreamSide,
    mut requester: Box<dyn ResendRequester>,
    mut callback: Option<FrameCallback>,
) -> std::io::Result<StreamHandle> {
    socket.set_read_timeout(Some(RECEIVE_POLL_TIMEOUT))?;
    let (stop_tx, stop_rx) = mpsc::channel();

    let join = thread::spawn(move || {
        let mut reassembler = Reassembler::new(cfg);
        let mut buf = [0u8; MAX_DATAGRAM_SIZE];
        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            let mut closed = match socket.recv(&mut buf) {
                Ok(n) => match GvspHeader::parse(&buf[..n]) {
                    Ok((status, header, payload)) => reassembler.process_packet(
                        status,
                        header,
                        payload,
                        &pool,
                        requester.as_mut(),
                        Instant::now(),
                    ),
                    Err(_) => Vec::new(),
                },
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    reassembler.tick(requester.as_mut(), Instant::now())
                }
                Err(_) => Vec::new(),
            };
            for buffer in closed.drain(..) {
                if let Some(cb) = callback.as_mut() {
                    cb(&buffer);
                }
                pool.push_output_buffer(buffer);
            }
        }
    });

    Ok(StreamHandle {
        stop_tx: Some(stop_tx),
        join: Some(join),
    })
}
