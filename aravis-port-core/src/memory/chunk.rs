use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChunkError {
    #[error("malformed chunk trailer at offset {0}")]
    Malformed(usize),
}

/// An index over a GigE Vision chunk payload: a reverse linked list of length-prefixed-from-the-end
/// TLV blocks — `[data][id:u32 BE][size:u32 BE]` repeated, where `size` covers only `data`. Built
/// once per buffer, then looked up by chunk id.
#[derive(Debug, Clone, Default)]
pub struct ChunkTlvIndex {
    offsets: HashMap<u32, (usize, usize)>,
}

impl ChunkTlvIndex {
    /// Walk `data` backward from the end, indexing each `(id, size)` trailer, as Aravis's
    /// `arv_buffer_get_chunk_data` does. In chunk mode a GigE Vision device wraps the image
    /// itself as the first chunk (confirmed on the live C6-2040-GigE), so `data` can be the whole
    /// received payload; every byte must belong to some chunk, since anything that doesn't would
    /// be misread as further trailers. Returns `ChunkError::Malformed` rather than panicking on a
    /// corrupt/adversarial trailer whose implied start would underflow.
    pub fn build(data: &[u8]) -> Result<Self, ChunkError> {
        let mut offsets = HashMap::new();
        let mut cursor = data.len();
        while cursor > 8 {
            let size = u32::from_be_bytes(data[cursor - 4..cursor].try_into().unwrap()) as usize;
            let id = u32::from_be_bytes(data[cursor - 8..cursor - 4].try_into().unwrap());
            let start = cursor
                .checked_sub(8 + size)
                .ok_or(ChunkError::Malformed(cursor))?;
            offsets.insert(id, (start, size));
            cursor = start;
        }
        Ok(Self { offsets })
    }

    pub fn get<'a>(&self, data: &'a [u8], id: u32) -> Option<&'a [u8]> {
        let (start, len) = *self.offsets.get(&id)?;
        data.get(start..start + len)
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.offsets.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tlv_block(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = payload.to_vec();
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        v
    }

    #[test]
    fn indexes_multiple_chunks_in_the_chunk_data_region() {
        // `build` only ever sees the chunk-data region (the caller slices off the leading image
        // payload first — `received_size - image_size` bytes, per the GVSP leader's declared
        // image size); it does not walk into unrelated pixel data.
        let mut full_buffer = Vec::new();
        full_buffer.extend_from_slice(b"IMAGE-PIXEL-DATA");
        let chunk_region_start = full_buffer.len();
        full_buffer.extend(tlv_block(1, b"chunk-one"));
        full_buffer.extend(tlv_block(2, b"c2"));

        let chunk_region = &full_buffer[chunk_region_start..];
        let index = ChunkTlvIndex::build(chunk_region).unwrap();
        assert_eq!(index.get(chunk_region, 1), Some(b"chunk-one".as_slice()));
        assert_eq!(index.get(chunk_region, 2), Some(b"c2".as_slice()));
        assert_eq!(index.get(chunk_region, 99), None);
    }

    #[test]
    fn chunk_right_at_start_of_buffer() {
        let data = tlv_block(5, b"only-chunk");
        let index = ChunkTlvIndex::build(&data).unwrap();
        assert_eq!(index.get(&data, 5), Some(b"only-chunk".as_slice()));
    }

    #[test]
    fn malformed_size_that_would_underflow_is_an_error_not_a_panic() {
        // size field claims more bytes than actually precede the trailer.
        let mut data = vec![0u8; 4];
        data.extend_from_slice(&1u32.to_be_bytes()); // id
        data.extend_from_slice(&1_000_000u32.to_be_bytes()); // size (way too large)
        assert!(matches!(
            ChunkTlvIndex::build(&data),
            Err(ChunkError::Malformed(_))
        ));
    }
}
