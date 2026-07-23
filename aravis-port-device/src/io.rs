use std::io;
use std::sync::{Arc, Mutex};

use aravis_port_genicam::RegisterAccess;
use aravis_port_net::GvcpTransaction;

/// Bridges `aravis-port-genicam`'s transport-agnostic `RegisterAccess` trait to the shared,
/// mutex-guarded GVCP transaction (shared with the heartbeat thread).
pub(crate) struct GvcpTransactionIo(pub Arc<Mutex<GvcpTransaction>>);

fn to_io_error(e: aravis_port_core::Error) -> io::Error {
    io::Error::other(e.to_string())
}

impl RegisterAccess for GvcpTransactionIo {
    fn read_memory(&mut self, address: u64, len: usize) -> io::Result<Vec<u8>> {
        self.0.lock().unwrap().read_memory(address as u32, len).map_err(to_io_error)
    }

    fn write_memory(&mut self, address: u64, data: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().write_memory(address as u32, data).map_err(to_io_error)
    }
}
