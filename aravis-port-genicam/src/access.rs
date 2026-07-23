/// The transport boundary between the GenICam engine and whatever moves bytes on/off the wire.
/// Keeping this trait tiny (and this crate free of any dependency on `aravis-port-net`) is what
/// lets the node tree, formula evaluator, and caching logic be tested with a plain `Vec<u8>`
/// stand-in instead of real sockets.
pub trait RegisterAccess {
    fn read_memory(&mut self, address: u64, len: usize) -> std::io::Result<Vec<u8>>;
    fn write_memory(&mut self, address: u64, data: &[u8]) -> std::io::Result<()>;
}

/// A simple in-memory `RegisterAccess` used by tests (and available to downstream crates for
/// their own tests) — no sockets, no threads.
#[derive(Debug, Default, Clone)]
pub struct MemoryRegisterAccess {
    pub bytes: Vec<u8>,
    pub read_count: usize,
}

impl MemoryRegisterAccess {
    pub fn new(size: usize) -> Self {
        Self {
            bytes: vec![0u8; size],
            read_count: 0,
        }
    }
}

impl RegisterAccess for MemoryRegisterAccess {
    fn read_memory(&mut self, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        self.read_count += 1;
        let start = address as usize;
        if start + len > self.bytes.len() {
            self.bytes.resize(start + len, 0);
        }
        Ok(self.bytes[start..start + len].to_vec())
    }

    fn write_memory(&mut self, address: u64, data: &[u8]) -> std::io::Result<()> {
        let start = address as usize;
        if start + data.len() > self.bytes.len() {
            self.bytes.resize(start + data.len(), 0);
        }
        self.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
}
