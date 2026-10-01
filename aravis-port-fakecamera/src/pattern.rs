/// Generates a simple diagonal-gradient Mono8 test pattern, enough to give a streaming test
/// something non-trivial (and frame-dependent) to verify.
pub fn generate_mono8(width: u32, height: u32, frame_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = (x.wrapping_add(y).wrapping_add(frame_id as u32) % 255) as u8;
            data.push(value);
        }
    }
    data
}

/// The same diagonal gradient as [`generate_mono8`], but as little-endian `u16` pixels for the
/// unpacked >8-bit formats (Mono10/Mono12/Mono16). The ramp is stretched by 8 per step so a
/// 16-bit frame actually exercises values above 255, then masked to `bits` the way a sensor with
/// that depth would never set the high bits.
pub fn generate_mono16(width: u32, height: u32, frame_id: u64, bits: u32) -> Vec<u8> {
    let mask: u32 = if bits >= 16 { 0xffff } else { (1u32 << bits) - 1 };
    let mut data = Vec::with_capacity((width * height * 2) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = (x.wrapping_add(y).wrapping_add(frame_id as u32).wrapping_mul(8) & mask) as u16;
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    data
}

/// The test pattern for whatever `PixelFormat` register value is currently set.
pub fn generate(width: u32, height: u32, frame_id: u64, pixel_format: u32) -> Vec<u8> {
    match pixel_format {
        crate::registers::pixel_format::MONO10 => generate_mono16(width, height, frame_id, 10),
        crate::registers::pixel_format::MONO16 => generate_mono16(width, height, frame_id, 16),
        _ => generate_mono8(width, height, frame_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::pixel_format;

    #[test]
    fn produces_the_expected_byte_count() {
        let data = generate_mono8(8, 4, 0);
        assert_eq!(data.len(), 32);
    }

    #[test]
    fn differs_across_frame_ids() {
        assert_ne!(generate_mono8(8, 4, 0), generate_mono8(8, 4, 1));
    }

    #[test]
    fn mono16_is_two_little_endian_bytes_per_pixel() {
        let data = generate_mono16(8, 4, 0, 16);
        assert_eq!(data.len(), 8 * 4 * 2);
        // Pixel (x=7, y=3) of frame 0 is (7 + 3) * 8 = 80.
        let last = u16::from_le_bytes([data[62], data[63]]);
        assert_eq!(last, 80);
    }

    #[test]
    fn mono16_exceeds_the_8_bit_range() {
        let data = generate_mono16(64, 64, 0, 16);
        let max = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).max().unwrap();
        assert!(max > 255, "max {max}");
    }

    #[test]
    fn mono10_never_sets_bits_above_ten() {
        let data = generate_mono16(256, 64, 3, 10);
        assert!(data.chunks_exact(2).all(|c| u16::from_le_bytes([c[0], c[1]]) < 1024));
    }

    #[test]
    fn the_register_value_picks_the_layout() {
        assert_eq!(generate(8, 4, 0, pixel_format::MONO8).len(), 32);
        assert_eq!(generate(8, 4, 0, pixel_format::MONO10).len(), 64);
        assert_eq!(generate(8, 4, 0, pixel_format::MONO16).len(), 64);
    }
}
