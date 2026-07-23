//! A minimal, safe, pure-Rust RFC 1951 DEFLATE decoder (raw deflate streams — no zlib/gzip
//! wrapper). Written from scratch for this project since no zip/flate2/miniz_oxide dependency is
//! in the allowed dependency list, and real GenICam cameras (confirmed against the live
//! AT-Automation Technology C5-2040-GigE) serve their XML inside a `.zip` file.
//!
//! Implements the classic canonical-Huffman decode table (count-per-length + symbols-sorted-by-
//! code array), the same shape as the public-domain reference algorithm ("puff"), reimplemented
//! here rather than ported.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InflateError {
    #[error("unexpected end of compressed data")]
    UnexpectedEnd,
    #[error("invalid block type {0}")]
    InvalidBlockType(u32),
    #[error("stored block length mismatch: len=0x{len:04x} ~len=0x{nlen:04x}")]
    StoredLengthMismatch { len: u16, nlen: u16 },
    #[error("invalid huffman code")]
    InvalidHuffmanCode,
    #[error("back-reference distance {distance} exceeds output length {output_len}")]
    InvalidDistance { distance: usize, output_len: usize },
    #[error("too many code-length codes")]
    TooManyCodeLengths,
}

type Result<T> = std::result::Result<T, InflateError>;

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u32> {
        let byte = *self.data.get(self.byte_pos).ok_or(InflateError::UnexpectedEnd)?;
        let bit = (byte >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(bit as u32)
    }

    fn read_bits(&mut self, n: u32) -> Result<u32> {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.read_bit()? << i;
        }
        Ok(v)
    }

    fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    fn read_u16_le(&mut self) -> Result<u16> {
        let lo = *self.data.get(self.byte_pos).ok_or(InflateError::UnexpectedEnd)?;
        let hi = *self.data.get(self.byte_pos + 1).ok_or(InflateError::UnexpectedEnd)?;
        self.byte_pos += 2;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let bytes = self
            .data
            .get(self.byte_pos..self.byte_pos + n)
            .ok_or(InflateError::UnexpectedEnd)?;
        self.byte_pos += n;
        Ok(bytes)
    }
}

/// A canonical Huffman decode table: `counts[len]` = number of symbols with that code length,
/// `symbols` = symbol values sorted by (code length, symbol index).
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn build(lengths: &[u8]) -> Self {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;

        let mut offsets = [0u16; 16];
        for len in 1..16 {
            offsets[len] = offsets[len - 1] + counts[len - 1];
        }

        let mut symbols = vec![0u16; lengths.iter().filter(|&&l| l != 0).count()];
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                symbols[offsets[len as usize] as usize] = sym as u16;
                offsets[len as usize] += 1;
            }
        }

        Self { counts, symbols }
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..16usize {
            code |= br.read_bit()? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::InvalidHuffmanCode)
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn fixed_huffman_tables() -> (Huffman, Huffman) {
    let mut lit_lengths = [0u8; 288];
    for (i, l) in lit_lengths.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist_lengths = [5u8; 30];
    (Huffman::build(&lit_lengths), Huffman::build(&dist_lengths))
}

fn read_dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman)> {
    let hlit = br.read_bits(5)? as usize + 257;
    let hdist = br.read_bits(5)? as usize + 1;
    let hclen = br.read_bits(4)? as usize + 4;
    if hlit > 288 || hdist > 32 || hclen > 19 {
        return Err(InflateError::TooManyCodeLengths);
    }

    let mut cl_lengths = [0u8; 19];
    for &idx in CODE_LENGTH_ORDER.iter().take(hclen) {
        cl_lengths[idx] = br.read_bits(3)? as u8;
    }
    let cl_huffman = Huffman::build(&cl_lengths);

    let mut lengths = Vec::with_capacity(hlit + hdist);
    while lengths.len() < hlit + hdist {
        let sym = cl_huffman.decode(br)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths.last().ok_or(InflateError::InvalidHuffmanCode)?;
                let repeat = br.read_bits(2)? + 3;
                for _ in 0..repeat {
                    lengths.push(prev);
                }
            }
            17 => {
                let repeat = br.read_bits(3)? + 3;
                lengths.extend(std::iter::repeat_n(0u8, repeat as usize));
            }
            18 => {
                let repeat = br.read_bits(7)? + 11;
                lengths.extend(std::iter::repeat_n(0u8, repeat as usize));
            }
            _ => return Err(InflateError::InvalidHuffmanCode),
        }
    }
    if lengths.len() != hlit + hdist {
        return Err(InflateError::InvalidHuffmanCode);
    }

    Ok((Huffman::build(&lengths[..hlit]), Huffman::build(&lengths[hlit..])))
}

fn inflate_block(br: &mut BitReader, lit: &Huffman, dist: &Huffman, out: &mut Vec<u8>) -> Result<()> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let idx = (sym - 257) as usize;
                let length = LENGTH_BASE[idx] as usize + br.read_bits(LENGTH_EXTRA[idx])? as usize;
                let dist_sym = dist.decode(br)? as usize;
                if dist_sym >= 30 {
                    return Err(InflateError::InvalidHuffmanCode);
                }
                let distance = DIST_BASE[dist_sym] as usize + br.read_bits(DIST_EXTRA[dist_sym])? as usize;
                if distance > out.len() {
                    return Err(InflateError::InvalidDistance {
                        distance,
                        output_len: out.len(),
                    });
                }
                let start = out.len() - distance;
                for i in 0..length {
                    let byte = out[start + i];
                    out.push(byte);
                }
            }
            _ => return Err(InflateError::InvalidHuffmanCode),
        }
    }
}

/// Decompress a raw DEFLATE stream (no zlib/gzip wrapper). `size_hint` pre-allocates the output
/// buffer if the caller already knows the uncompressed size (e.g. from a ZIP local file header).
pub fn inflate(data: &[u8], size_hint: Option<usize>) -> Result<Vec<u8>> {
    let mut br = BitReader::new(data);
    let mut out = Vec::with_capacity(size_hint.unwrap_or(data.len() * 3));

    loop {
        let bfinal = br.read_bits(1)?;
        let btype = br.read_bits(2)?;
        match btype {
            0 => {
                br.align_to_byte();
                let len = br.read_u16_le()?;
                let nlen = br.read_u16_le()?;
                if len != !nlen {
                    return Err(InflateError::StoredLengthMismatch { len, nlen });
                }
                out.extend_from_slice(br.read_bytes(len as usize)?);
            }
            1 => {
                let (lit, dist) = fixed_huffman_tables();
                inflate_block(&mut br, &lit, &dist, &mut out)?;
            }
            2 => {
                let (lit, dist) = read_dynamic_tables(&mut br)?;
                inflate_block(&mut br, &lit, &dist, &mut out)?;
            }
            other => return Err(InflateError::InvalidBlockType(other)),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_block_round_trips() {
        // bfinal=1, btype=00 -> 0b001 in the low 3 bits of the first byte; rest of that byte is
        // padding to the next byte boundary, then LEN/NLEN (LE u16) + raw bytes.
        let payload = b"hello";
        let mut data = vec![0x01u8];
        data.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        data.extend_from_slice(&(!(payload.len() as u16)).to_le_bytes());
        data.extend_from_slice(payload);
        assert_eq!(inflate(&data, None).unwrap(), payload);
    }

    #[test]
    fn stored_block_length_mismatch_is_an_error_not_a_panic() {
        let data = vec![0x01u8, 0x05, 0x00, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o'];
        assert!(inflate(&data, None).is_err());
    }

    #[test]
    fn malformed_input_does_not_panic() {
        assert!(inflate(&[], None).is_err());
        assert!(inflate(&[0xff, 0xff, 0xff], None).is_err());
    }

    /// A real dynamic-huffman-compressed stream generated by Python's `zlib.compressobj(9,
    /// DEFLATED, -15)` (raw deflate, no wrapper) over a repeated XML snippet — exercises the
    /// dynamic Huffman table path (code-length alphabet, repeat codes 16/17/18, back-references)
    /// against a real encoder's output, not just hand-crafted bit patterns.
    #[test]
    fn dynamic_huffman_block_from_a_real_encoder() {
        const COMPRESSED: &[u8] = &[
            179, 9, 74, 77, 207, 44, 46, 73, 45, 114, 73, 45, 78, 46, 202, 44, 40, 201, 204, 207, 179, 179, 113, 78,
            44, 73, 77, 207, 47, 170, 84, 240, 75, 204, 77, 181, 85, 10, 202, 207, 47, 81, 178, 179, 41, 112, 75, 77,
            44, 41, 45, 74, 181, 11, 207, 76, 41, 201, 176, 209, 135, 243, 109, 244, 97, 26, 236, 108, 60, 243, 128,
            172, 212, 34, 168, 78, 176, 74, 160, 214, 176, 196, 156, 210, 84, 59, 51, 19, 27, 125, 8, 203, 70, 31,
            170, 14, 200, 194, 234, 130, 81, 103, 145, 224, 44, 0,
        ];
        let expected = "<RegisterDescription><Category Name=\"Root\"><pFeature>Width</pFeature></Category><Integer Name=\"Width\"><Value>64</Value></Integer></RegisterDescription>".repeat(3);
        let decompressed = inflate(COMPRESSED, Some(expected.len())).unwrap();
        assert_eq!(String::from_utf8(decompressed).unwrap(), expected);
    }
}
