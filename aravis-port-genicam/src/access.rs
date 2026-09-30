use aravis_port_core::memory::ChunkTlvIndex;

/// The transport boundary between the GenICam engine and whatever moves bytes on/off the wire.
/// Keeping this trait tiny (and this crate free of any dependency on `aravis-port-device`/networking) is what
/// lets the node tree, formula evaluator, and caching logic be tested with a plain `Vec<u8>`
/// stand-in instead of real sockets.
pub trait RegisterAccess {
    fn read_memory(&mut self, address: u64, len: usize) -> std::io::Result<Vec<u8>>;
    fn write_memory(&mut self, address: u64, data: &[u8]) -> std::io::Result<()>;

    /// Read `len` bytes at `address` within chunk `chunk_id` of the buffer currently being
    /// inspected. Only registers behind a chunk port use this; the default has no buffer
    /// attached, so reading a chunk feature through plain device access is an error.
    fn read_chunk(&mut self, chunk_id: u32, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let _ = (address, len);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("chunk 0x{chunk_id:08x}: no buffer attached (read chunk features through a chunk-data accessor)"),
        ))
    }
}

/// Adds an acquired buffer's chunk data to another `RegisterAccess`: chunk-port registers read
/// from the buffer, everything else (e.g. a device register a chunk feature's address formula
/// depends on) still goes to `inner`. The Aravis counterpart is `ArvChunkParser`.
pub struct ChunkDataAccess<'a, R> {
    inner: R,
    data: &'a [u8],
    index: ChunkTlvIndex,
}

impl<'a, R: RegisterAccess> ChunkDataAccess<'a, R> {
    /// `payload` is the buffer's received payload. GigE Vision chunk data is a sequence of
    /// `[data][id: u32 BE][size: u32 BE]` blocks ending at the end of the payload, the image
    /// itself being the first such block.
    pub fn new(inner: R, payload: &'a [u8]) -> std::io::Result<Self> {
        let index = ChunkTlvIndex::build(payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Self {
            inner,
            data: payload,
            index,
        })
    }
}

impl<R: RegisterAccess> RegisterAccess for ChunkDataAccess<'_, R> {
    fn read_memory(&mut self, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        self.inner.read_memory(address, len)
    }

    fn write_memory(&mut self, address: u64, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_memory(address, data)
    }

    fn read_chunk(&mut self, chunk_id: u32, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let chunk = self.index.get(self.data, chunk_id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("chunk 0x{chunk_id:08x} not in buffer"))
        })?;
        usize::try_from(address)
            .ok()
            .and_then(|start| chunk.get(start..start.checked_add(len)?))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("chunk 0x{chunk_id:08x}: {len} bytes at {address} exceed its {} bytes", chunk.len()),
                )
            })
    }
}

impl<R: RegisterAccess + ?Sized> RegisterAccess for &mut R {
    fn read_memory(&mut self, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        (**self).read_memory(address, len)
    }

    fn write_memory(&mut self, address: u64, data: &[u8]) -> std::io::Result<()> {
        (**self).write_memory(address, data)
    }

    fn read_chunk(&mut self, chunk_id: u32, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        (**self).read_chunk(chunk_id, address, len)
    }
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
