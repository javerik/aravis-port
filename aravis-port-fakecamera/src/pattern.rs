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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_the_expected_byte_count() {
        let data = generate_mono8(8, 4, 0);
        assert_eq!(data.len(), 32);
    }

    #[test]
    fn differs_across_frame_ids() {
        assert_ne!(generate_mono8(8, 4, 0), generate_mono8(8, 4, 1));
    }
}
